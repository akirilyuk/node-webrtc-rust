//! Default `SttProvider` output methods: a provider that only implements the original methods is
//! an Inline provider whose `poll_output` maps `poll_transcript` and whose `request_finalize`
//! runs `finalize_utterance`.

use std::collections::VecDeque;

use bytes::Bytes;
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SttOutput, SttProvider, SttTranscript, DEFAULT_STT_FINALIZE_WAIT_MS,
};
use node_webrtc_rust_speech::SpeechResult;

#[derive(Default)]
struct LegacyStt {
    queue: VecDeque<SttTranscript>,
    finalize_calls: usize,
}

#[async_trait::async_trait]
impl SttProvider for LegacyStt {
    fn vendor_name(&self) -> &'static str {
        "legacy"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        self.queue.push_back(SttTranscript::Partial("alpha".into()));
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.queue.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        self.finalize_calls += 1;
        self.queue.push_back(SttTranscript::Final("alpha".into()));
        Ok(())
    }
}

#[tokio::test]
async fn legacy_provider_is_inline_and_maps_poll_transcript() {
    let mut stt = LegacyStt::default();
    assert_eq!(stt.finalize_mode(), FinalizeMode::Inline);
    assert_eq!(stt.finalize_wait_ms(), DEFAULT_STT_FINALIZE_WAIT_MS);

    stt.push_audio(Bytes::from_static(&[0; 640])).await.unwrap();
    stt.request_finalize().await.unwrap();
    assert_eq!(stt.finalize_calls, 1);

    let mut out = Vec::new();
    while let Some(o) = stt.poll_output().await.unwrap() {
        out.push(o);
    }
    assert_eq!(
        out,
        vec![
            SttOutput::Transcript(SttTranscript::Partial("alpha".into())),
            SttOutput::Transcript(SttTranscript::Final("alpha".into())),
        ],
        "the default never invents a FinalizeDone marker"
    );
}
