//! gRPC cluster-sherpa vendor — streams STT/TTS to in-cluster speech-service.

mod channel;
mod factory;
pub mod metrics;
mod stt;
mod tts;

pub use factory::ClusterSherpaFactory;
pub use metrics::{inc_stt_reopen, reset_stt_reopen_metrics, stt_reopen_total};
pub use stt::ClusterSherpaStt;
pub use tts::ClusterSherpaTts;
