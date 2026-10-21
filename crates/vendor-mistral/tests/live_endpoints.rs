//! Which Mistral endpoint to use: the three documented ones are probed with the documented
//! `GET {base}/v1/models` (which of our model ids it serves) and timed with 5 TCP+TLS connects
//! to host:443. No billed call is made. Run with `--features live`.
//!
//! Needs `MISTRAL_API_KEY`. Without it the test prints a note and returns, unless
//! `MISTRAL_LIVE_REQUIRED=1`, which turns a missing key into a panic. The key is never printed.
//! A 401 or 403 from an endpoint means "not available to this key": it is reported, not fatal.
//!
//! Output, one line per endpoint, then the choice:
//! `mistral_endpoint {"label":..,"url":..,"status":..,"models":[..],"connect_ms_median":..,"connect_ms":[..]}`
//! `mistral_endpoint_choice {"label":..,"url":..,"reason":..}`
//!
//! The rule (`choose_endpoint`): of the endpoints that serve the default TTS and the default STT
//! model, the one with the lowest median connect time. If none does, the lowest-median endpoint
//! that serves the default STT model plus any TTS model, with the served TTS model named in the
//! reason. If none does either, a named error.

use node_webrtc_rust_vendor_mistral::{
    DEFAULT_STT_MODEL, DEFAULT_TTS_MODEL, DOCUMENTED_TTS_MODELS,
};

/// One probed endpoint.
#[derive(Debug, Clone)]
struct Probe {
    label: String,
    url: String,
    /// HTTP status of `GET /v1/models`; 0 when no response arrived.
    status: u16,
    /// Our model ids (TTS and STT) found in the response's `data[].id`.
    served: Vec<String>,
    /// `None` when no connect sample succeeded.
    connect_median_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    label: String,
    url: String,
    reason: String,
}

fn serves(probe: &Probe, model: &str) -> bool {
    probe.served.iter().any(|m| m == model)
}

fn fastest<'a>(probes: impl Iterator<Item = &'a Probe>) -> Option<&'a Probe> {
    probes.min_by_key(|p| p.connect_median_ms.unwrap_or(u64::MAX))
}

fn choose_endpoint(probes: &[Probe]) -> Result<Choice, String> {
    let tts = *DEFAULT_TTS_MODEL;
    let stt = *DEFAULT_STT_MODEL;
    if let Some(p) = fastest(probes.iter().filter(|p| serves(p, tts) && serves(p, stt))) {
        return Ok(Choice {
            label: p.label.clone(),
            url: p.url.clone(),
            reason: format!(
                "serves {tts} and {stt}; lowest median connect {} ms",
                p.connect_median_ms.map_or("?".into(), |ms| ms.to_string())
            ),
        });
    }
    let any_tts = |p: &Probe| DOCUMENTED_TTS_MODELS.iter().find(|m| serves(p, m)).copied();
    if let Some(p) = fastest(
        probes
            .iter()
            .filter(|p| serves(p, stt) && any_tts(p).is_some()),
    ) {
        let served_tts = any_tts(p).expect("filtered");
        return Ok(Choice {
            label: p.label.clone(),
            url: p.url.clone(),
            reason: format!(
                "no endpoint serves {tts} and {stt}; this one serves {stt} and TTS model {served_tts}; lowest median connect {} ms",
                p.connect_median_ms.map_or("?".into(), |ms| ms.to_string())
            ),
        });
    }
    Err(format!(
        "no Mistral endpoint serves the default STT model {stt} together with a TTS model; statuses: {}",
        probes
            .iter()
            .map(|p| format!("{}={}", p.label, p.status))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn median(samples: &[u64]) -> Option<u64> {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied()
}

#[cfg(feature = "live")]
mod network {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use node_webrtc_rust_vendor_mistral::ENDPOINTS;
    use node_webrtc_rust_voice_catalog as voice_catalog;
    use serde_json::{json, Value};
    use tokio::net::TcpStream;

    use super::{choose_endpoint, median, Probe};

    const BOUND: Duration = Duration::from_secs(10);
    const CONNECT_SAMPLES: usize = 5;

    /// Our five model ids: the catalog TTS models and STT models.
    fn our_models() -> Vec<&'static str> {
        voice_catalog::tts_models("mistral")
            .expect("tts models")
            .iter()
            .chain(voice_catalog::stt_models("mistral").expect("stt models"))
            .copied()
            .collect()
    }

    /// `GET {base}/v1/models`, documented list-models shape: `{"data": [{"id": ..}, ..]}`.
    async fn list_models(base: &str, key: &str) -> (u16, Vec<String>) {
        let client = reqwest::Client::builder()
            .timeout(BOUND)
            .build()
            .expect("client");
        let response = match client
            .get(format!("{base}/v1/models"))
            .bearer_auth(key)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                println!("mistral_models_error {base} {}", err.without_url());
                return (0, Vec::new());
            }
        };
        let status = response.status().as_u16();
        if !response.status().is_success() {
            // 401/403: not available to this key. Reported, not fatal.
            return (status, Vec::new());
        }
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(err) => {
                println!(
                    "mistral_models_error {base} unreadable body: {}",
                    err.without_url()
                );
                return (status, Vec::new());
            }
        };
        let ids = body
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        (status, ids)
    }

    fn tls_connector() -> tokio_rustls::TlsConnector {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        tokio_rustls::TlsConnector::from(Arc::new(config))
    }

    /// One TCP connect plus TLS handshake to `host:443`, in ms.
    async fn connect_once(connector: &tokio_rustls::TlsConnector, host: &str) -> Option<u64> {
        let started = Instant::now();
        let attempt = async {
            let tcp = TcpStream::connect((host, 443)).await.ok()?;
            let name = rustls::pki_types::ServerName::try_from(host.to_string()).ok()?;
            connector.connect(name, tcp).await.ok()?;
            Some(())
        };
        tokio::time::timeout(BOUND, attempt).await.ok()??;
        Some(started.elapsed().as_millis() as u64)
    }

    pub async fn probe_all(key: &str) -> Vec<Probe> {
        let ours = our_models();
        let connector = tls_connector();
        let mut probes = Vec::new();
        for (label, url) in ENDPOINTS {
            let host = url.trim_start_matches("https://");
            let (status, listed) = list_models(url, key).await;
            let mut samples = Vec::new();
            for _ in 0..CONNECT_SAMPLES {
                if let Some(ms) = connect_once(&connector, host).await {
                    samples.push(ms);
                }
            }
            let served: Vec<String> = ours
                .iter()
                .filter(|m| listed.iter().any(|id| id == *m))
                .map(|m| m.to_string())
                .collect();
            let median_ms = median(&samples);
            println!(
                "mistral_endpoint {}",
                json!({
                    "label": label,
                    "url": url,
                    "status": status,
                    "models": served,
                    "connect_ms_median": median_ms,
                    "connect_ms": samples,
                })
            );
            probes.push(Probe {
                label: label.to_string(),
                url: url.to_string(),
                status,
                served,
                connect_median_ms: median_ms,
            });
        }
        probes
    }

    pub fn print_choice(probes: &[Probe]) {
        match choose_endpoint(probes) {
            Ok(choice) => println!(
                "mistral_endpoint_choice {}",
                json!({"label": choice.label, "url": choice.url, "reason": choice.reason})
            ),
            Err(err) => panic!("{err}"),
        }
    }
}

#[cfg(feature = "live")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoints_probe_and_choice() {
    use node_webrtc_rust_vendor_live_support::env::live_env;
    let Some(mut env) = live_env("mistral", &["MISTRAL_API_KEY"]) else {
        return;
    };
    let key = env.remove("MISTRAL_API_KEY").expect("key");
    let probes = network::probe_all(&key).await;
    network::print_choice(&probes);
}

// ---- the rule -----------------------------------------------------------------------------

fn probe(label: &str, median_ms: Option<u64>, served: &[&str]) -> Probe {
    Probe {
        label: label.into(),
        url: format!("https://{label}.example"),
        status: 200,
        served: served.iter().map(|m| m.to_string()).collect(),
        connect_median_ms: median_ms,
    }
}

fn all_defaults() -> Vec<&'static str> {
    vec![*DEFAULT_TTS_MODEL, *DEFAULT_STT_MODEL]
}

#[test]
fn all_serve_all_the_fastest_wins() {
    let probes = [
        probe("global", Some(180), &all_defaults()),
        probe("eu", Some(40), &all_defaults()),
        probe("us", Some(300), &all_defaults()),
    ];
    let choice = choose_endpoint(&probes).unwrap();
    assert_eq!(choice.label, "eu");
    assert!(choice.reason.contains("40 ms"), "{}", choice.reason);
}

#[test]
fn fastest_without_the_default_tts_model_loses_to_the_next() {
    let probes = [
        probe("global", Some(180), &all_defaults()),
        probe("eu", Some(40), &["voxtral-mini-tts-3", *DEFAULT_STT_MODEL]),
        probe("us", Some(300), &all_defaults()),
    ];
    assert_eq!(choose_endpoint(&probes).unwrap().label, "global");
}

#[test]
fn none_serves_all_falls_back_and_names_the_tts_model() {
    let probes = [
        probe("global", Some(180), &[*DEFAULT_TTS_MODEL]),
        probe("eu", Some(60), &["voxtral-mini-tts-3", *DEFAULT_STT_MODEL]),
        probe("us", Some(30), &["voxtral-mini-tts-3"]),
    ];
    let choice = choose_endpoint(&probes).unwrap();
    assert_eq!(choice.label, "eu");
    assert!(
        choice.reason.contains("TTS model voxtral-mini-tts-3;")
            && choice.reason.contains("no endpoint"),
        "{}",
        choice.reason
    );
}

#[test]
fn none_serves_anything_is_a_named_error() {
    let mut denied = probe("global", Some(50), &[]);
    denied.status = 403;
    let probes = [denied, probe("eu", Some(40), &[]), probe("us", None, &[])];
    let err = choose_endpoint(&probes).unwrap_err();
    assert!(
        err.contains("no Mistral endpoint") && err.contains("global=403"),
        "{err}"
    );
}

#[test]
fn median_of_five_is_the_middle_sample() {
    assert_eq!(median(&[90, 10, 50, 30, 70]), Some(50));
    assert_eq!(median(&[]), None);
}
