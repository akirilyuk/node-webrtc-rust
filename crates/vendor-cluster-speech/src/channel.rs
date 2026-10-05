use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use tonic::transport::Channel;

const CHANNEL_IDLE_EVICT: Duration = Duration::from_secs(600);

/// Max decoded gRPC message size on speech clients (tonic default is 4 MiB).
pub(crate) const MAX_GRPC_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

struct CachedChannel {
    channel: Channel,
    last_used: Instant,
}

static STT_CHANNELS: OnceLock<Mutex<HashMap<String, CachedChannel>>> = OnceLock::new();
static TTS_CHANNELS: OnceLock<Mutex<HashMap<String, CachedChannel>>> = OnceLock::new();

fn stt_channels() -> &'static Mutex<HashMap<String, CachedChannel>> {
    STT_CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn tts_channels() -> &'static Mutex<HashMap<String, CachedChannel>> {
    TTS_CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn evict_stale(map: &mut HashMap<String, CachedChannel>) {
    let now = Instant::now();
    map.retain(|_, entry| now.duration_since(entry.last_used) < CHANNEL_IDLE_EVICT);
}

pub fn resolve_stt_endpoint(config: &SttConfig) -> SpeechResult<String> {
    if let Some(endpoint) = config.endpoint.as_ref() {
        let trimmed = endpoint.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(url) = std::env::var("SPEECH_STT_SERVICE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(url) = std::env::var("SPEECH_SERVICE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    Err(SpeechError::Config(
        "no speech endpoint for cluster-sherpa".into(),
    ))
}

pub fn resolve_tts_endpoint(config: &TtsConfig) -> SpeechResult<String> {
    if let Some(endpoint) = config.endpoint.as_ref() {
        let trimmed = endpoint.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(url) = std::env::var("SPEECH_TTS_SERVICE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(url) = std::env::var("SPEECH_SERVICE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    Err(SpeechError::Config(
        "no speech endpoint for cluster-sherpa".into(),
    ))
}

pub fn resolve_speech_token(config_key: &Option<String>) -> Option<String> {
    config_key
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            std::env::var("SPEECH_SERVICE_TOKEN")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

async fn connect_endpoint(endpoint: &str) -> SpeechResult<Channel> {
    let channel = Channel::from_shared(endpoint.to_string())
        .map_err(|e| SpeechError::Config(format!("invalid speech endpoint: {e}")))?
        .http2_keep_alive_interval(Duration::from_secs(20))
        .keep_alive_timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(3))
        .tcp_nodelay(true)
        .connect()
        .await
        .map_err(|e| SpeechError::Vendor {
            vendor: "cluster-sherpa".into(),
            message: format!("connect {endpoint}: {e}"),
        })?;
    Ok(channel)
}

pub async fn stt_channel(endpoint: &str) -> SpeechResult<Channel> {
    {
        let mut map = stt_channels().lock().expect("stt channel cache");
        evict_stale(&mut map);
        if let Some(entry) = map.get(endpoint) {
            let ch = entry.channel.clone();
            map.get_mut(endpoint).expect("entry").last_used = Instant::now();
            return Ok(ch);
        }
    }
    let channel = connect_endpoint(endpoint).await?;
    let mut map = stt_channels().lock().expect("stt channel cache");
    map.insert(
        endpoint.to_string(),
        CachedChannel {
            channel: channel.clone(),
            last_used: Instant::now(),
        },
    );
    Ok(channel)
}

pub async fn tts_channel(endpoint: &str) -> SpeechResult<Channel> {
    {
        let mut map = tts_channels().lock().expect("tts channel cache");
        evict_stale(&mut map);
        if let Some(entry) = map.get(endpoint) {
            let ch = entry.channel.clone();
            map.get_mut(endpoint).expect("entry").last_used = Instant::now();
            return Ok(ch);
        }
    }
    let channel = connect_endpoint(endpoint).await?;
    let mut map = tts_channels().lock().expect("tts channel cache");
    map.insert(
        endpoint.to_string(),
        CachedChannel {
            channel: channel.clone(),
            last_used: Instant::now(),
        },
    );
    Ok(channel)
}
