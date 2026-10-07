//! gRPC cluster-sherpa vendor — streams STT/TTS to in-cluster speech-service.

mod channel;
mod factory;
pub mod metrics;
mod stt;
mod tts;

pub use factory::ClusterSherpaFactory;
pub use metrics::{
    inc_stt_reopen, inc_stt_utterance_streams, record_stt_stream_open_ms, reset_stt_reopen_metrics,
    stt_reopen_total, stt_stream_open_ms_buckets, stt_stream_open_ms_stats,
    stt_utterance_streams_total, STREAM_OPEN_BUCKETS_MS,
};
pub use stt::{
    stream_per_utterance_from_env, ClusterSherpaStt, ClusterSttOptions, STREAM_PER_UTTERANCE_ENV,
};
pub use tts::ClusterSherpaTts;
