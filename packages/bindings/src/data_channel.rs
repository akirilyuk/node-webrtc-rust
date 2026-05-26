//! RTCDataChannel NAPI bindings.

use std::sync::Arc;

use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use napi::JsFunction;
use napi::JsUnknown;
use node_webrtc_rust_core::{DataChannel, DataChannelMessage, DataChannelOptions, DataChannelState};
use tokio::sync::{mpsc, Mutex};

use crate::config::{core_err, to_js_unknown};
use crate::events::{
    create_error_callback, create_event_callback, create_void_callback, wire_error_channel,
    wire_event_channel, wire_void_channel,
};

/// WebRTC data channel exposed to JavaScript.
#[napi]
pub struct JsRTCDataChannel {
    inner: Arc<DataChannel>,
    open_wired: Mutex<bool>,
    message_wired: Mutex<bool>,
    close_wired: Mutex<bool>,
    error_wired: Mutex<bool>,
}

impl JsRTCDataChannel {
    pub(crate) fn new(inner: DataChannel) -> Self {
        Self {
            inner: Arc::new(inner),
            open_wired: Mutex::new(false),
            message_wired: Mutex::new(false),
            close_wired: Mutex::new(false),
            error_wired: Mutex::new(false),
        }
    }
}

#[napi]
impl JsRTCDataChannel {
    #[napi(getter)]
    pub fn label(&self) -> String {
        self.inner.label().to_string()
    }

    #[napi(getter)]
    pub fn id(&self) -> u16 {
        self.inner.id()
    }

    #[napi(getter)]
    pub fn ready_state(&self) -> String {
        match self.inner.ready_state() {
            DataChannelState::Connecting => "connecting".to_string(),
            DataChannelState::Open => "open".to_string(),
            DataChannelState::Closing => "closing".to_string(),
            DataChannelState::Closed => "closed".to_string(),
        }
    }

    #[napi]
    pub async fn buffered_amount(&self) -> Result<u32> {
        Ok(self.inner.buffered_amount().await as u32)
    }

    #[napi]
    pub async fn send(&self, data: Either<String, Buffer>) -> Result<()> {
        match data {
            Either::A(text) => self.inner.send_text(&text).await.map_err(core_err),
            Either::B(buffer) => {
                let bytes = Bytes::copy_from_slice(buffer.as_ref());
                self.inner.send_binary(bytes).await.map_err(core_err)
            }
        }
    }

    #[napi]
    pub async fn close(&self) -> Result<()> {
        self.inner.close().await.map_err(core_err)
    }

    #[napi]
    pub fn set_on_open(&self, env: Env, callback: JsFunction) -> Result<()> {
        let mut wired = self.open_wired.blocking_lock();
        if *wired {
            return Ok(());
        }
        *wired = true;

        let (tx, rx) = mpsc::unbounded_channel();
        let tsfn = create_void_callback(&env, callback)?;
        wire_void_channel(rx, tsfn);

        let inner = Arc::clone(&self.inner);
        inner.on_open(move || {
            let _ = tx.send(());
        });

        Ok(())
    }

    #[napi]
    pub fn set_on_message(&self, env: Env, callback: JsFunction) -> Result<()> {
        let mut wired = self.message_wired.blocking_lock();
        if *wired {
            return Ok(());
        }
        *wired = true;

        let (tx, rx) = mpsc::unbounded_channel();
        let tsfn = create_event_callback(&env, callback, |ctx| -> Result<Vec<JsUnknown>> {
            let message: DataChannelMessage = ctx.value;
            if message.is_string {
                let text = std::str::from_utf8(&message.data).map_err(|err| {
                    Error::from_reason(format!("invalid UTF-8 in data channel message: {err}"))
                })?;
                to_js_unknown(&ctx.env, ctx.env.create_string(text)?)
                    .map(|value| vec![value])
            } else {
                Ok(vec![
                    ctx.env
                        .create_buffer_with_data(message.data.to_vec())?
                        .into_unknown(),
                ])
            }
        })?;
        wire_event_channel(rx, tsfn);

        let inner = Arc::clone(&self.inner);
        inner.on_message(move |message| {
            let _ = tx.send(message);
        });

        Ok(())
    }

    #[napi]
    pub fn set_on_close(&self, env: Env, callback: JsFunction) -> Result<()> {
        let mut wired = self.close_wired.blocking_lock();
        if *wired {
            return Ok(());
        }
        *wired = true;

        let (tx, rx) = mpsc::unbounded_channel();
        let tsfn = create_void_callback(&env, callback)?;
        wire_void_channel(rx, tsfn);

        let inner = Arc::clone(&self.inner);
        inner.on_close(move || {
            let _ = tx.send(());
        });

        Ok(())
    }

    #[napi]
    pub fn set_on_error(&self, env: Env, callback: JsFunction) -> Result<()> {
        let mut wired = self.error_wired.blocking_lock();
        if *wired {
            return Ok(());
        }
        *wired = true;

        let (tx, rx) = mpsc::unbounded_channel();
        let tsfn = create_error_callback(&env, callback)?;
        wire_error_channel(rx, tsfn);

        let inner = Arc::clone(&self.inner);
        inner.on_error(move |err| {
            let _ = tx.send(err.to_string());
        });

        Ok(())
    }
}

/// Options for creating a data channel from JavaScript.
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRTCDataChannelInit {
    pub ordered: Option<bool>,
    pub max_packet_life_time: Option<u16>,
    pub max_retransmits: Option<u16>,
    pub protocol: Option<String>,
    pub negotiated: Option<u16>,
}

impl From<JsRTCDataChannelInit> for DataChannelOptions {
    fn from(value: JsRTCDataChannelInit) -> Self {
        Self {
            ordered: value.ordered,
            max_packet_life_time: value.max_packet_life_time,
            max_retransmits: value.max_retransmits,
            protocol: value.protocol,
            negotiated: value.negotiated,
        }
    }
}
