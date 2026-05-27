//! Local audio track that accepts PCM and encodes to the negotiated RTP codec.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use media::Sample;
use tokio::sync::Mutex;
use webrtc::api::media_engine::MIME_TYPE_OPUS;
use webrtc::error::Result as WebRtcResult;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::{TrackLocal, TrackLocalContext};

use crate::debug_call;
use crate::error::CoreError;
use crate::pcm_encoder::{NegotiatedAudioFormat, PcmEncoder};

/// Underlying track local: PCM in, negotiated codec out.
pub struct PcmAudioTrackLocal {
    sample: Arc<TrackLocalStaticSample>,
    negotiated: Arc<Mutex<Option<NegotiatedAudioFormat>>>,
    encoder: PcmEncoder,
}

impl PcmAudioTrackLocal {
    pub fn new(id: &str, stream_id: &str) -> Result<Self, CoreError> {
        let sample = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                ..Default::default()
            },
            id.to_owned(),
            stream_id.to_owned(),
        ));

        Ok(Self {
            sample,
            negotiated: Arc::new(Mutex::new(None)),
            encoder: PcmEncoder::new()?,
        })
    }

    pub fn id(&self) -> &str {
        self.sample.id()
    }

    pub fn stream_id(&self) -> &str {
        self.sample.stream_id()
    }

    /// Returns the negotiated audio format after SDP bind, if available.
    pub async fn negotiated_format(&self) -> Option<NegotiatedAudioFormat> {
        self.negotiated.lock().await.clone()
    }

    /// Encodes PCM to the negotiated codec and writes one RTP sample.
    pub async fn write_pcm_sample(
        &self,
        pcm: Bytes,
        duration: Duration,
    ) -> Result<(), CoreError> {
        debug_call!(
            "core::pcm_audio_track",
            "write_pcm_sample",
            "id={}, pcm_bytes={}, duration_ms={}",
            self.id(),
            pcm.len(),
            duration.as_millis()
        );

        let format = self
            .negotiated
            .lock()
            .await
            .clone()
            .unwrap_or_else(NegotiatedAudioFormat::advertised_opus);

        let (payload, duration) = self.encoder.encode(&format, &pcm, duration)?;
        debug_call!(
            "core::pcm_audio_track",
            "write_pcm_sample",
            "id={}, codec={}, payload_bytes={}",
            self.id(),
            format.mime_type,
            payload.len()
        );

        let sample = Sample {
            data: payload,
            duration,
            ..Default::default()
        };

        self.sample
            .write_sample(&sample)
            .await
            .map_err(|e| CoreError::Track(e.to_string()))
    }
}

#[async_trait]
impl TrackLocal for PcmAudioTrackLocal {
    async fn bind(&self, context: &TrackLocalContext) -> WebRtcResult<RTCRtpCodecParameters> {
        let codec = self.sample.bind(context).await?;
        match NegotiatedAudioFormat::from_codec(&codec) {
            Ok(format) => {
                debug_call!(
                    "core::pcm_audio_track",
                    "bind",
                    "id={}, codec={}",
                    self.id(),
                    format.mime_type
                );
                *self.negotiated.lock().await = Some(format);
            }
            Err(error) => {
                debug_call!(
                    "core::pcm_audio_track",
                    "bind",
                    "id={}, codec_parse_error={error}",
                    self.id()
                );
            }
        }
        Ok(codec)
    }

    async fn unbind(&self, context: &TrackLocalContext) -> WebRtcResult<()> {
        self.sample.unbind(context).await?;
        *self.negotiated.lock().await = None;
        Ok(())
    }

    fn id(&self) -> &str {
        self.sample.id()
    }

    fn rid(&self) -> Option<&str> {
        self.sample.rid()
    }

    fn stream_id(&self) -> &str {
        self.sample.stream_id()
    }

    fn kind(&self) -> RTPCodecType {
        self.sample.kind()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
