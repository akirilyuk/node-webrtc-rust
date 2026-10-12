//! `voice_mix_frames_total{outcome}` OpenTelemetry counter.
//!
//! With the `otel` feature the counter is created on the global meter provider (installed by
//! `node-webrtc-rust-speech`'s `init_from_env`, same meter name as the voice metrics). Without
//! it every call is an inlined no-op. The only label is `outcome`; there is no peer, session
//! or participant label.

/// What happened to a source frame, mirroring the `mixStats` counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixFrameOutcome {
    Played,
    Compressed,
    Concealed,
    OverflowCompressed,
    DecodeError,
}

#[cfg(feature = "otel")]
mod enabled {
    use super::MixFrameOutcome;
    use opentelemetry::{global, metrics::Counter, KeyValue};
    use std::sync::OnceLock;

    struct Instruments {
        counter: Counter<u64>,
        attrs: [[KeyValue; 1]; 5],
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
