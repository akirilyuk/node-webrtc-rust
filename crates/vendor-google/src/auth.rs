//! Credentials of the Google paths, and which path accepts which.
//!
//! REST paths (V1 `speech:recognize`, `text:synthesize`), first match wins:
//! 1. an explicit `config.api_key` (sent as `?key=`),
//! 2. `GOOGLE_API_KEY` (sent as `?key=`),
//! 3. `GOOGLE_ACCESS_TOKEN` (bearer),
//! 4. the service account file named by `GOOGLE_APPLICATION_CREDENTIALS` (bearer).
//!
//! gRPC paths (V2 `StreamingRecognize`, Chirp 3 HD `StreamingSynthesize`) need OAuth: only the
//! service account file named by `GOOGLE_APPLICATION_CREDENTIALS`. The Google pages read for this
//! crate (Speech-to-Text and Text-to-Speech authentication, the streaming guides, the REST
//! references, which list only the OAuth scope `https://www.googleapis.com/auth/cloud-platform`)
//! document no API-key authentication for these gRPC methods, so a key alone is refused with a
//! named config error instead of being sent. Application Default Credentials outside that env var
//! (gcloud user credentials, the metadata server) are never read.

#[cfg(feature = "live")]
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

/// How a REST request authenticates.
#[cfg(feature = "live")]
pub(crate) enum RestAuth {
    ApiKey(String),
    Bearer(String),
}

/// A non-empty trimmed value.
#[cfg(feature = "live")]
pub(crate) fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// `GOOGLE_APPLICATION_CREDENTIALS`, read when a provider is built.
#[cfg(feature = "live")]
pub(crate) fn credentials_file_from_env() -> Option<String> {
    clean(
        std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .as_deref(),
    )
}

#[cfg(feature = "live")]
pub(crate) async fn rest_auth(config_api_key: Option<&str>) -> SpeechResult<RestAuth> {
    if let Some(key) = clean(config_api_key) {
        return Ok(RestAuth::ApiKey(key));
    }
    if let Some(key) = google_api_key() {
        return Ok(RestAuth::ApiKey(key));
    }
    let token = google_access_token()
        .await?
        .ok_or_else(|| SpeechError::Vendor {
            vendor: "google".into(),
            message: "failed to obtain Google access token".into(),
        })?;
    Ok(RestAuth::Bearer(token))
}

#[cfg(feature = "live")]
async fn google_access_token() -> SpeechResult<Option<String>> {
    if let Some(token) = clean(std::env::var("GOOGLE_ACCESS_TOKEN").ok().as_deref()) {
        return Ok(Some(token));
    }

    let credentials_path = credentials_file_from_env().ok_or_else(|| {
        SpeechError::Config(
            "missing Google credentials: set an API key (config apiKey or GOOGLE_API_KEY), \
             GOOGLE_ACCESS_TOKEN, or GOOGLE_APPLICATION_CREDENTIALS"
                .into(),
        )
    })?;

    let key_json = tokio::fs::read_to_string(&credentials_path)
        .await
        .map_err(|err| {
            SpeechError::Config(format!("read GOOGLE_APPLICATION_CREDENTIALS: {err}"))
        })?;
    let key: yup_oauth2::ServiceAccountKey = serde_json::from_str(&key_json)
        .map_err(|_| SpeechError::Config("parse service account JSON failed".into()))?;
    let auth = yup_oauth2::ServiceAccountAuthenticator::builder(key)
        .build()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    let token = auth
        .token(&["https://www.googleapis.com/auth/cloud-platform"])
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;
    Ok(token.token().map(|t| t.to_string()))
}

#[cfg(feature = "live")]
fn google_api_key() -> Option<String> {
    clean(std::env::var("GOOGLE_API_KEY").ok().as_deref())
}

/// The named error for a gRPC path without a service account. `what` is e.g.
/// `stt chirp_3`, `rest_paths` e.g. `latest_long and Neural2 voices`.
#[cfg(feature = "live")]
pub(crate) fn needs_service_account(what: &str, rest_paths: &str) -> SpeechError {
    SpeechError::Config(format!(
        "google {what} needs a service account (GOOGLE_APPLICATION_CREDENTIALS) — \
         API keys work only for {rest_paths}"
    ))
}

/// OAuth credentials for the gRPC clients, from the service account file only.
#[cfg(feature = "live")]
pub(crate) async fn grpc_credentials(
    file: Option<&str>,
    what: &str,
    rest_paths: &str,
) -> SpeechResult<google_cloud_auth::credentials::Credentials> {
    let Some(path) = file else {
        return Err(needs_service_account(what, rest_paths));
    };
    let json = tokio::fs::read_to_string(path).await.map_err(|err| {
        SpeechError::Config(format!("read GOOGLE_APPLICATION_CREDENTIALS: {err}"))
    })?;
    // Parse errors never echo the file (it holds a private key).
    let value: serde_json::Value = serde_json::from_str(&json)
        .map_err(|_| SpeechError::Config("parse service account JSON failed".into()))?;
    google_cloud_auth::credentials::service_account::Builder::new(value)
        .build()
        .map_err(|_| {
            SpeechError::Config(
                "GOOGLE_APPLICATION_CREDENTIALS is not a valid service account key".into(),
            )
        })
}
