//! `voice_mix_frames_total{outcome}` OpenTelemetry counter.
//!
//! With the `otel` feature the counter is created on the global meter provider (same meter name
//! as the voice metrics). The bindings start OTel (`node-webrtc-rust-speech`'s `init_from_env`)
//! when an `AudioMixGraph` or a conference room is created, because the cached instrument must
//! be built after the provider is installed: an instrument created earlier stays a no-op.
//! Without the feature every call is an inlined no-op. The only label is `outcome`; there is no
//! peer, session or participant label.

/// What happened to a source frame, mirroring the `mixStats` counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixFrameOutcome {
    Played,
    Compressed,
    Concealed,
    OverflowCompressed,
    DecodeError,
    ListenerCatchupCompressed,
    ListenerSkipped,
}

#[cfg(feature = "otel")]
mod enabled {
    use super::MixFrameOutcome;
    use opentelemetry::{global, metrics::Counter, KeyValue};
    use std::sync::OnceLock;

    struct Instruments {
        counter: Counter<u64>,
        attrs: [[KeyValue; 1]; 7],
    }

    static INSTRUMENTS: OnceLock<Instruments> = OnceLock::new();

    fn instruments() -> &'static Instruments {
        INSTRUMENTS.get_or_init(|| Instruments {
            // Instrument "voice_mix_frames" exports as `voice_mix_frames_total` (same as
            // "voice_barge_in" -> `voice_barge_in_total`).
            counter: global::meter("node-webrtc-rust-speech")
                .u64_counter("voice_mix_frames")
                .with_description("Mixer source frames by outcome")
                .build(),
            attrs: [
                [KeyValue::new("outcome", "played")],
                [KeyValue::new("outcome", "compressed")],
                [KeyValue::new("outcome", "concealed")],
                [KeyValue::new("outcome", "overflow_compressed")],
                [KeyValue::new("outcome", "decode_error")],
                [KeyValue::new("outcome", "listener_catchup_compressed")],
                [KeyValue::new("outcome", "listener_skipped")],
            ],
        })
    }

    pub(crate) fn record_mix_frames(outcome: MixFrameOutcome, n: u64) {
        let i = instruments();
        i.counter.add(n, &i.attrs[outcome as usize]);
    }
}

#[cfg(feature = "otel")]
pub(crate) use enabled::record_mix_frames;

#[cfg(not(feature = "otel"))]
#[inline(always)]
pub(crate) fn record_mix_frames(_outcome: MixFrameOutcome, _n: u64) {}
