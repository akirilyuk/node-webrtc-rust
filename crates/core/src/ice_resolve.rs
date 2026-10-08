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
pub(crate) async fn resolve_ice_servers(servers: Vec<IceServer>) -> Vec<IceServer> {
    resolve_with(servers, default_lookup, resolve_timeout(), global_cache()).await
}

async fn default_lookup(hostport: String) -> io::Result<Vec<SocketAddr>> {
    Ok(tokio::net::lookup_host(hostport).await?.collect())
}

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
            let outcome = match tokio::time::timeout(timeout, resolver(key.clone())).await {
                Ok(Ok(addrs)) => pick(&addrs).ok_or_else(|| "no addresses".to_owned()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("timed out".to_owned()),
            };
            (key, outcome)
        });
    }
    let mut failures: HashMap<String, String> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((key, outcome)) = joined {
            match outcome {
                Ok(ip) => cache.put(key, Some(ip)),
                Err(reason) => {
                    cache.put(key.clone(), None);
                    failures.insert(key, reason);
                }
            }
        }
    }

    let ms = timeout.as_millis();
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
            match cache.get(&cache_key(&parsed)) {
                Some(Some(ip)) => urls.push(rewrite_ice_url(&parsed, ip)),
                _ => {
                    let reason = failures
                        .get(&cache_key(&parsed))
                        .map(String::as_str)
                        .unwrap_or("recent resolution failure");
                    log::warn!("ice server {url} not resolved within {ms} ms: {reason} — skipped");
                }
            }
        }
        if !urls.is_empty() {
            server.urls = urls;
            out.push(server);
        }
    }
    out
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

    #[tokio::test]
    async fn resolver_that_never_answers_is_dropped_within_the_bound() {
        let cache = Cache::default();
        let start = Instant::now();
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
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].urls, vec!["stun:1.2.3.4:3478"]);
    }

    #[tokio::test]
    async fn two_dead_hosts_cost_one_timeout() {
        let cache = Cache::default();
        let start = Instant::now();
        let out = resolve_with(
            vec![server("stun:a.example:3478"), server("turn:b.example:3478")],
            never,
            Duration::from_millis(300),
            &cache,
        )
        .await;
        assert!(out.is_empty());
        assert!(start.elapsed() < Duration::from_millis(600));
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

    #[tokio::test]
    async fn negative_cache_skips_the_wait() {
        let cache = Cache::default();
        let servers = vec![server("stun:dead.example:3478")];
        let first = resolve_with(servers.clone(), never, Duration::from_millis(200), &cache).await;
        assert!(first.is_empty());
        let start = Instant::now();
        let second = resolve_with(servers, never, Duration::from_millis(200), &cache).await;
        assert!(second.is_empty());
        assert!(start.elapsed() < Duration::from_millis(50));
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
