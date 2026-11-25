//! Bounded DNS resolution of STUN/TURN hostnames.
//!
//! webrtc-ice resolves ICE server hostnames with `lookup_host` and no timeout, so a stalled
//! resolver hangs ICE gathering. We resolve first, with a bound, and hand webrtc-ice IP literals.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::IceServer;

const DEFAULT_RESOLVE_TIMEOUT: Duration = Duration::from_millis(2_000);
const POSITIVE_TTL: Duration = Duration::from_secs(60);
const NEGATIVE_TTL: Duration = Duration::from_secs(30);

/// Resolve timeout: `WEBRTC_ICE_RESOLVE_TIMEOUT_MS` (clamped 100..=10000) or 2 s.
fn resolve_timeout() -> Duration {
    match std::env::var("WEBRTC_ICE_RESOLVE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(ms) => Duration::from_millis(ms.clamp(100, 10_000)),
        None => DEFAULT_RESOLVE_TIMEOUT,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    Stun,
    Stuns,
    Turn,
    Turns,
}

impl Scheme {
    fn as_str(self) -> &'static str {
        match self {
            Scheme::Stun => "stun",
            Scheme::Stuns => "stuns",
            Scheme::Turn => "turn",
            Scheme::Turns => "turns",
        }
    }

    fn is_tls(self) -> bool {
        matches!(self, Scheme::Stuns | Scheme::Turns)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IceUrl {
    pub scheme: Scheme,
    pub host: String,
    pub port: Option<u16>,
    pub query: Option<String>,
}

pub(crate) fn parse_ice_url(url: &str) -> Option<IceUrl> {
    let (scheme_str, rest) = url.split_once(':')?;
    let scheme = match scheme_str.to_ascii_lowercase().as_str() {
        "stun" => Scheme::Stun,
        "stuns" => Scheme::Stuns,
        "turn" => Scheme::Turn,
        "turns" => Scheme::Turns,
        _ => return None,
    };
    let (authority, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q.to_owned())),
        None => (rest, None),
    };
    if authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let (h, after) = stripped.split_once(']')?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        (h.to_owned(), port)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => {
                if p.contains(':') {
                    return None;
                }
                (h.to_owned(), Some(p.parse::<u16>().ok()?))
            }
            None => (authority.to_owned(), None),
        }
    };
    if host.is_empty() {
        return None;
    }
    Some(IceUrl {
        scheme,
        host,
        port,
        query,
    })
}

pub(crate) fn rewrite_ice_url(url: &IceUrl, ip: IpAddr) -> String {
    let host = match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    };
    let mut out = format!("{}:{host}", url.scheme.as_str());
    if let Some(port) = url.port {
        out.push_str(&format!(":{port}"));
    }
    if let Some(q) = &url.query {
        out.push('?');
        out.push_str(q);
    }
    out
}

/// Lookups per host before its urls are dropped: one retry after a timeout or any resolver
/// error other than an explicit no-such-name answer (worst case 2 x the bound).
const RESOLVE_ATTEMPTS: u32 = 2;

enum Lookup {
    Resolved(IpAddr),
    /// Not cached: the next peer connection looks the host up again.
    Transient(String),
    /// Cached for `NEGATIVE_TTL`.
    Definitive(String),
}

/// Definitive only for an explicit "no such name" answer (glibc/musl/macOS EAI_NONAME,
/// glibc EAI_NODATA). Any other resolver error gets the retry and is not cached: std reports
/// every getaddrinfo failure as an uncategorised io error, and the EAI_AGAIN text differs by
/// libc (glibc "Temporary failure in name resolution", musl "Try again").
fn is_definitive(e: &io::Error) -> bool {
    let m = e.to_string();
    [
        "Name or service not known",
        "No address associated with hostname",
        "Name does not resolve",
        "nodename nor servname provided",
    ]
    .iter()
    .any(|s| m.contains(s))
}

/// Resolution cache keyed by `host:port`; `None` means a recent failure.
#[derive(Default)]
pub(crate) struct Cache(Mutex<HashMap<String, (Option<IpAddr>, Instant)>>);

impl Cache {
    fn get(&self, key: &str) -> Option<Option<IpAddr>> {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key) {
            Some((ip, at)) => {
                let ttl = if ip.is_some() {
                    POSITIVE_TTL
                } else {
                    NEGATIVE_TTL
                };
                if at.elapsed() < ttl {
                    Some(*ip)
                } else {
                    map.remove(key);
                    None
                }
            }
            None => None,
        }
    }

    fn put(&self, key: String, ip: Option<IpAddr>) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(key, (ip, Instant::now()));
    }
}

fn global_cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

fn needs_resolution(url: &IceUrl) -> bool {
    !url.scheme.is_tls() && url.host.parse::<IpAddr>().is_err() && url.host != "localhost"
}

fn cache_key(url: &IceUrl) -> String {
    format!("{}:{}", url.host, url.port.unwrap_or(3478))
}

fn pick(addrs: &[SocketAddr]) -> Option<IpAddr> {
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .map(|a| a.ip())
}

/// Resolves hostnames in `servers` with a bound; unresolvable urls are dropped.
///
/// Every dropped url is written to stderr (always on, so container logs carry it) and to
/// `log::warn!`. A dropped STUN server can leave a peer without a reachable srflx candidate,
/// which looks like "every ICE pair failed" with no other trace (staging 2026-11-25).
pub(crate) async fn resolve_ice_servers(servers: Vec<IceServer>) -> Vec<IceServer> {
    let (out, skipped) =
        resolve_with_notices(servers, default_lookup, resolve_timeout(), global_cache()).await;
    for notice in &skipped {
        eprintln!("{notice}");
        log::warn!("{notice}");
    }
    out
}

/// One line per url dropped by [`resolve_with_notices`]. Carries no credentials: ICE
/// usernames and passwords are separate fields, never part of the url.
pub(crate) fn skip_notice(url: &str, timeout_ms: u128, reason: &str) -> String {
    format!("[webrtc ice_resolve] ice server {url} not resolved within {timeout_ms} ms: {reason} — skipped")
}

async fn default_lookup(hostport: String) -> io::Result<Vec<SocketAddr>> {
    Ok(tokio::net::lookup_host(hostport).await?.collect())
}

#[cfg(test)]
pub(crate) async fn resolve_with<F, Fut>(
    servers: Vec<IceServer>,
    resolver: F,
    timeout: Duration,
    cache: &Cache,
) -> Vec<IceServer>
where
    F: Fn(String) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'static,
{
    resolve_with_notices(servers, resolver, timeout, cache)
        .await
        .0
}

/// Like `resolve_with`, plus one [`skip_notice`] per dropped url.
pub(crate) async fn resolve_with_notices<F, Fut>(
    servers: Vec<IceServer>,
    resolver: F,
    timeout: Duration,
    cache: &Cache,
) -> (Vec<IceServer>, Vec<String>)
where
    F: Fn(String) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'static,
{
    // Collect distinct keys not in the cache and resolve them concurrently.
    let mut pending: HashMap<String, ()> = HashMap::new();
    for server in &servers {
        for url in &server.urls {
            if let Some(parsed) = parse_ice_url(url) {
                if needs_resolution(&parsed) {
                    let key = cache_key(&parsed);
                    if cache.get(&key).is_none() {
                        pending.insert(key, ());
                    }
                }
            }
        }
    }

    let mut set = tokio::task::JoinSet::new();
    for key in pending.into_keys() {
        let resolver = resolver.clone();
        set.spawn(async move {
            let mut attempt = 1;
            let outcome = loop {
                let result = match tokio::time::timeout(timeout, resolver(key.clone())).await {
                    Ok(Ok(addrs)) => match pick(&addrs) {
                        Some(ip) => Lookup::Resolved(ip),
                        None => Lookup::Definitive("no addresses".to_owned()),
                    },
                    Ok(Err(e)) if is_definitive(&e) => Lookup::Definitive(e.to_string()),
                    Ok(Err(e)) => Lookup::Transient(e.to_string()),
                    Err(_) => Lookup::Transient("timed out".to_owned()),
                };
                match result {
                    Lookup::Transient(_) if attempt < RESOLVE_ATTEMPTS => attempt += 1,
                    Lookup::Transient(reason) => {
                        break Lookup::Transient(format!("{reason} ({attempt} attempts)"))
                    }
                    done => break done,
                }
            };
            (key, outcome)
        });
    }
    // This call's own outcomes. Only definitive ones go into the shared cache: a lost UDP
    // DNS packet must not drop the server from every connect in the process for NEGATIVE_TTL.
    let mut resolved: HashMap<String, IpAddr> = HashMap::new();
    let mut failures: HashMap<String, String> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((key, outcome)) = joined {
            match outcome {
                Lookup::Resolved(ip) => {
                    cache.put(key.clone(), Some(ip));
                    resolved.insert(key, ip);
                }
                Lookup::Definitive(reason) => {
                    cache.put(key.clone(), None);
                    failures.insert(key, reason);
                }
                Lookup::Transient(reason) => {
                    failures.insert(key, reason);
                }
            }
        }
    }

    let ms = timeout.as_millis();
    let mut skipped = Vec::new();
    let mut out = Vec::with_capacity(servers.len());
    for mut server in servers {
        let mut urls = Vec::with_capacity(server.urls.len());
        for url in std::mem::take(&mut server.urls) {
            let Some(parsed) = parse_ice_url(&url) else {
                urls.push(url);
                continue;
            };
            if !needs_resolution(&parsed) {
                urls.push(url);
                continue;
            }
            let key = cache_key(&parsed);
            let hit = resolved.get(&key).copied().or_else(|| {
                if failures.contains_key(&key) {
                    None
                } else {
                    cache.get(&key).flatten()
                }
            });
            match hit {
                Some(ip) => urls.push(rewrite_ice_url(&parsed, ip)),
                _ => {
                    let reason = failures
                        .get(&cache_key(&parsed))
                        .map(String::as_str)
                        .unwrap_or("recent resolution failure");
                    skipped.push(skip_notice(&url, ms, reason));
                }
            }
        }
        if !urls.is_empty() {
            server.urls = urls;
            out.push(server);
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn server(url: &str) -> IceServer {
        IceServer {
            urls: vec![url.to_owned()],
            ..Default::default()
        }
    }

    async fn never(_: String) -> io::Result<Vec<SocketAddr>> {
        std::future::pending().await
    }

    #[test]
    fn parse_and_rewrite_table() {
        let u = parse_ice_url("stun:stun.l.google.com:19302").unwrap();
        assert_eq!(u.scheme, Scheme::Stun);
        assert_eq!(u.host, "stun.l.google.com");
        assert_eq!(u.port, Some(19302));

        let t = parse_ice_url("turn:turn.example.com:3478?transport=udp").unwrap();
        assert_eq!(
            rewrite_ice_url(&t, "1.2.3.4".parse().unwrap()),
            "turn:1.2.3.4:3478?transport=udp"
        );

        let v6 = parse_ice_url("stun:[::1]:3478").unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.port, Some(3478));
        assert_eq!(
            rewrite_ice_url(&v6, "::1".parse().unwrap()),
            "stun:[::1]:3478"
        );

        let tls = parse_ice_url("turns:x:5349").unwrap();
        assert!(!needs_resolution(&tls));

        assert!(parse_ice_url("garbage").is_none());
        assert!(parse_ice_url("http://x").is_none());
        assert!(parse_ice_url("stun:").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_url_yields_one_notice_naming_it() {
        let cache = Cache::default();
        let (out, skipped) = resolve_with_notices(
            vec![
                server("stun:stalled.example:19302"),
                server("stun:1.2.3.4:3478"),
            ],
            never,
            Duration::from_millis(100),
            &cache,
        )
        .await;
        assert_eq!(out.len(), 1);
        assert_eq!(
            skipped,
            vec![skip_notice(
                "stun:stalled.example:19302",
                100,
                "timed out (2 attempts)"
            )]
        );
        assert!(
            skipped[0].starts_with("[webrtc ice_resolve] ice server stun:stalled.example:19302")
        );
        assert!(skipped[0].ends_with("— skipped"));
    }

    #[tokio::test]
    async fn resolved_and_literal_urls_yield_no_notice() {
        let cache = Cache::default();
        let (out, skipped) = resolve_with_notices(
            vec![server("stun:ok.example:3478"), server("stun:1.2.3.4:3478")],
            |_h: String| async { Ok(vec!["5.6.7.8:3478".parse().unwrap()]) },
            Duration::from_millis(100),
            &cache,
        )
        .await;
        assert_eq!(out.len(), 2);
        assert!(skipped.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn resolver_that_never_answers_is_dropped_within_the_bound() {
        let cache = Cache::default();
        let start = tokio::time::Instant::now();
        let out = resolve_with(
            vec![
                server("stun:dead.example:3478"),
                server("stun:1.2.3.4:3478"),
            ],
            never,
            Duration::from_millis(200),
            &cache,
        )
        .await;
        assert_eq!(start.elapsed(), Duration::from_millis(400));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].urls, vec!["stun:1.2.3.4:3478"]);
    }

    #[tokio::test(start_paused = true)]
    async fn two_dead_hosts_cost_one_bounded_retry() {
        let cache = Cache::default();
        let start = tokio::time::Instant::now();
        let out = resolve_with(
            vec![server("stun:a.example:3478"), server("turn:b.example:3478")],
            never,
            Duration::from_millis(300),
            &cache,
        )
        .await;
        assert!(out.is_empty());
        // Hosts resolve concurrently; each gets 2 attempts of 300 ms.
        assert_eq!(start.elapsed(), Duration::from_millis(600));
    }

    #[tokio::test]
    async fn resolved_host_is_rewritten_to_ip() {
        let cache = Cache::default();
        let out = resolve_with(
            vec![server("stun:stun.example:3478")],
            |_| async { Ok(vec!["10.0.0.5:3478".parse().unwrap()]) },
            Duration::from_millis(200),
            &cache,
        )
        .await;
        assert_eq!(out[0].urls, vec!["stun:10.0.0.5:3478"]);
    }

    type LookupFut = std::pin::Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;

    /// Resolver that answers from a script, one entry per call, and counts calls.
    fn scripted(
        script: Vec<&'static str>,
    ) -> (
        Arc<AtomicUsize>,
        impl Fn(String) -> LookupFut + Clone + Send + Sync + 'static,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let script = Arc::new(script);
        let f = move |_h: String| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            let step = script.get(n).copied().unwrap_or("stall");
            let fut: LookupFut = match step {
                "ok" => Box::pin(async { Ok(vec!["9.9.9.9:19302".parse().unwrap()]) }),
                "nxdomain" => Box::pin(async {
                    Err(io::Error::other(
                        "failed to lookup address information: Name or service not known",
                    ))
                }),
                "again" => Box::pin(async {
                    Err(io::Error::other(
                            "failed to lookup address information: Temporary failure in name resolution",
                        ))
                }),
                "musl_again" => Box::pin(async {
                    Err(io::Error::other(
                        "failed to lookup address information: Try again",
                    ))
                }),
                "musl_nx" => Box::pin(async {
                    Err(io::Error::other(
                        "failed to lookup address information: Name does not resolve",
                    ))
                }),
                "unknown" => Box::pin(async {
                    Err(io::Error::other(
                        "failed to lookup address information: Unknown error",
                    ))
                }),
                _ => Box::pin(std::future::pending()),
            };
            fut
        };
        (calls, f)
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_then_ok_keeps_the_url_after_two_attempts() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["stall", "ok"]);
        let (out, skipped) = resolve_with_notices(
            vec![server("stun:stun.example:19302")],
            resolver,
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert_eq!(out[0].urls, vec!["stun:9.9.9.9:19302"]);
        assert!(skipped.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_error_then_ok_keeps_the_url() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["again", "ok"]);
        let out = resolve_with(
            vec![server("stun:stun.example:19302")],
            resolver,
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert_eq!(out[0].urls, vec!["stun:9.9.9.9:19302"]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_twice_drops_with_one_notice_after_two_attempts() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["stall", "stall"]);
        let start = tokio::time::Instant::now();
        let (out, skipped) = resolve_with_notices(
            vec![server("stun:stun.example:19302")],
            resolver,
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert!(out.is_empty());
        assert_eq!(
            skipped,
            vec![skip_notice(
                "stun:stun.example:19302",
                2_000,
                "timed out (2 attempts)"
            )]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(start.elapsed(), Duration::from_millis(4_000));
    }

    #[tokio::test(start_paused = true)]
    async fn nxdomain_is_not_retried_and_is_cached() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["nxdomain", "ok"]);
        let servers = vec![server("stun:gone.example:3478")];
        let first = resolve_with(
            servers.clone(),
            resolver.clone(),
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert!(first.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let second = resolve_with(servers, resolver, Duration::from_millis(2_000), &cache).await;
        assert!(second.is_empty());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "definitive failure is negative-cached"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn musl_try_again_is_retried_and_not_cached() {
        let servers = vec![server("stun:stun.example:19302")];
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["musl_again", "ok"]);
        let out = resolve_with(
            servers.clone(),
            resolver,
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert_eq!(out[0].urls, vec!["stun:9.9.9.9:19302"]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["musl_again", "musl_again", "ok"]);
        let first = resolve_with(
            servers.clone(),
            resolver.clone(),
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert!(first.is_empty());
        let second = resolve_with(servers, resolver, Duration::from_millis(2_000), &cache).await;
        assert_eq!(second[0].urls, vec!["stun:9.9.9.9:19302"]);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn unrecognised_resolver_error_is_retried() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["unknown", "ok"]);
        let out = resolve_with(
            vec![server("stun:stun.example:19302")],
            resolver,
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert_eq!(out[0].urls, vec!["stun:9.9.9.9:19302"]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn musl_nxdomain_is_not_retried_and_is_cached() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["musl_nx", "ok"]);
        let servers = vec![server("stun:gone.example:3478")];
        let first = resolve_with(
            servers.clone(),
            resolver.clone(),
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert!(first.is_empty());
        let second = resolve_with(servers, resolver, Duration::from_millis(2_000), &cache).await;
        assert!(second.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_is_not_cached_so_the_next_resolve_looks_up_again() {
        let cache = Cache::default();
        let (calls, resolver) = scripted(vec!["stall", "stall", "ok"]);
        let servers = vec![server("stun:stun.example:19302")];
        let first = resolve_with(
            servers.clone(),
            resolver.clone(),
            Duration::from_millis(2_000),
            &cache,
        )
        .await;
        assert!(first.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let second = resolve_with(servers, resolver, Duration::from_millis(2_000), &cache).await;
        assert_eq!(second[0].urls, vec!["stun:9.9.9.9:19302"]);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn ip_literal_and_tls_urls_are_untouched() {
        let cache = Cache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let servers = vec![
            server("stun:1.2.3.4:3478"),
            server("stun:[::1]:3478"),
            server("stun:localhost:3478"),
            server("stuns:host.example:5349"),
            server("turns:host.example:5349"),
        ];
        let out = resolve_with(
            servers,
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                async { Ok(vec![]) }
            },
            Duration::from_millis(200),
            &cache,
        )
        .await;
        assert_eq!(out.len(), 5);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(out[3].urls, vec!["stuns:host.example:5349"]);
    }
}
