//! DataChannel wrapper around webrtc-rs.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use webrtc::data_channel::RTCDataChannel;

use crate::debug_call;
use crate::debug_evt;
use crate::error::{is_benign_teardown_error, CoreError};

/// Options for creating a DataChannel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DataChannelOptions {
    /// Whether messages are delivered in order.
    pub ordered: Option<bool>,
    /// Maximum packet lifetime in milliseconds.
    pub max_packet_life_time: Option<u16>,
    /// Maximum retransmission attempts.
    pub max_retransmits: Option<u16>,
    /// Subprotocol name.
    pub protocol: Option<String>,
    /// Negotiated channel ID (out-of-band negotiation).
    pub negotiated: Option<u16>,
}

impl From<DataChannelOptions> for RTCDataChannelInit {
    fn from(options: DataChannelOptions) -> Self {
        RTCDataChannelInit {
            ordered: options.ordered,
            max_packet_life_time: options.max_packet_life_time,
            max_retransmits: options.max_retransmits,
            protocol: options.protocol,
            negotiated: options.negotiated,
        }
    }
}

/// DataChannel ready state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataChannelState {
    Connecting,
    Open,
    Closing,
    Closed,
}

impl From<RTCDataChannelState> for DataChannelState {
    fn from(state: RTCDataChannelState) -> Self {
        match state {
            RTCDataChannelState::Connecting => Self::Connecting,
            RTCDataChannelState::Open => Self::Open,
            RTCDataChannelState::Closing => Self::Closing,
            RTCDataChannelState::Closed => Self::Closed,
            _ => Self::Closed,
        }
    }
}

/// A message received on a DataChannel.
#[derive(Debug, Clone)]
pub struct DataChannelMessage {
    /// Whether the payload is UTF-8 text.
    pub is_string: bool,
    /// Shared payload bytes (zero-copy from webrtc-rs).
    pub data: Bytes,
}

/// Maximum number of messages kept while no `on_message` handler is set.
///
/// An app that never reads must not grow memory without bound; beyond this the
/// oldest messages are dropped.
const MAX_BUFFERED_MESSAGES: usize = 4096;

type MessageHandler = Arc<dyn Fn(DataChannelMessage) + Send + Sync>;

/// Where incoming messages go.
enum MessageSink {
    /// No app handler yet: keep messages in arrival order.
    Buffering {
        queue: VecDeque<DataChannelMessage>,
        warned: bool,
    },
    Forwarding(MessageHandler),
}

impl MessageSink {
    fn new() -> Self {
        Self::Buffering {
            queue: VecDeque::new(),
            warned: false,
        }
    }

    /// Delivers `msg` to the handler, or buffers it (newest `MAX_BUFFERED_MESSAGES` kept).
    fn push(&mut self, label: &str, msg: DataChannelMessage) {
        match self {
            Self::Forwarding(h) => h(msg),
            Self::Buffering { queue, warned } => {
                if queue.len() >= MAX_BUFFERED_MESSAGES {
                    queue.pop_front();
                    if !*warned {
                        *warned = true;
                        log::warn!(
                            "data channel {label}: {MAX_BUFFERED_MESSAGES} messages buffered before a handler was set; dropping oldest"
                        );
                    }
                }
                queue.push_back(msg);
            }
        }
    }

    /// Flushes buffered messages to `handler` in order, then forwards live ones.
    fn set_handler(&mut self, handler: MessageHandler) {
        if let Self::Buffering { queue, .. } = self {
            for msg in std::mem::take(queue) {
                handler(msg);
            }
        }
        *self = Self::Forwarding(handler);
    }
}

/// WebRTC DataChannel wrapper.
pub struct DataChannel {
    inner: Arc<RTCDataChannel>,
    messages: Arc<Mutex<MessageSink>>,
}

impl DataChannel {
    /// Wraps a native channel. Must be called once per native channel: it registers
    /// the native `on_message` hook that buffers until [`Self::on_message`] is called.
    pub(crate) fn from_inner(inner: Arc<RTCDataChannel>) -> Self {
        let messages = Arc::new(Mutex::new(MessageSink::new()));
        let sink = Arc::clone(&messages);
        let label = inner.label().to_owned();
        inner.on_message(Box::new(move |msg| {
            debug_evt!(
                "core::data_channel",
                "message",
                "is_string={}, bytes={}",
                msg.is_string,
                msg.data.len()
            );
            let msg = DataChannelMessage {
                is_string: msg.is_string,
                data: msg.data,
            };
            // Handler runs under the lock so ordering against `on_message` is strict.
            sink.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(&label, msg);
            Box::pin(async {})
        }));
        Self { inner, messages }
    }

    /// Returns the channel label.
    pub fn label(&self) -> &str {
        self.inner.label()
    }

    /// Returns the negotiated channel ID.
    pub fn id(&self) -> u16 {
        self.inner.id()
    }

    /// Returns the current ready state.
    pub fn ready_state(&self) -> DataChannelState {
        self.inner.ready_state().into()
    }

    /// Returns the number of bytes buffered for sending.
    pub async fn buffered_amount(&self) -> usize {
        self.inner.buffered_amount().await
    }

    /// Sends a UTF-8 text message.
    pub async fn send_text(&self, text: &str) -> Result<(), CoreError> {
        debug_call!(
            "core::data_channel",
            "send",
            "label={}, bytes={}",
            self.label(),
            text.len()
        );
        self.ensure_open_for_send()?;
        match self.inner.send_text(text.to_owned()).await {
            Ok(_) => Ok(()),
            Err(err) if is_benign_teardown_error(&err) => Ok(()),
            Err(err) => Err(CoreError::DataChannel(err.to_string())),
        }
    }

    /// Sends a binary message without copying when `data` is already `Bytes`.
    pub async fn send_binary(&self, data: Bytes) -> Result<(), CoreError> {
        debug_call!(
            "core::data_channel",
            "send",
            "label={}, bytes={}",
            self.label(),
            data.len()
        );
        self.ensure_open_for_send()?;
        match self.inner.send(&data).await {
            Ok(_) => Ok(()),
            Err(err) if is_benign_teardown_error(&err) => Ok(()),
            Err(err) => Err(CoreError::DataChannel(err.to_string())),
        }
    }

    /// Sends a binary slice, copying once into a shared buffer.
    pub async fn send_binary_slice(&self, data: &[u8]) -> Result<(), CoreError> {
        self.send_binary(Bytes::copy_from_slice(data)).await
    }

    /// Closes the channel.
    pub async fn close(&self) -> Result<(), CoreError> {
        debug_call!("core::data_channel", "close", "label={}", self.label());
        if matches!(
            self.ready_state(),
            DataChannelState::Closed | DataChannelState::Closing
        ) {
            return Ok(());
        }
        match self.inner.close().await {
            Ok(()) => Ok(()),
            Err(err) if is_benign_teardown_error(&err) => Ok(()),
            Err(err) => Err(CoreError::DataChannel(err.to_string())),
        }
    }

    fn ensure_open_for_send(&self) -> Result<(), CoreError> {
        match self.ready_state() {
            DataChannelState::Open => Ok(()),
            DataChannelState::Connecting => Err(CoreError::InvalidState(format!(
                "data channel '{}' is not open yet",
                self.label()
            ))),
            DataChannelState::Closing | DataChannelState::Closed => {
                Err(CoreError::InvalidState(format!(
                    "data channel '{}' is {}",
                    self.label(),
                    self.ready_state_name()
                )))
            }
        }
    }

    fn ready_state_name(&self) -> &'static str {
        match self.ready_state() {
            DataChannelState::Connecting => "connecting",
            DataChannelState::Open => "open",
            DataChannelState::Closing => "closing",
            DataChannelState::Closed => "closed",
        }
    }

    /// Registers a handler invoked when the channel opens.
    pub fn on_open(&self, handler: impl FnOnce() + Send + 'static) {
        let handler = std::sync::Mutex::new(Some(handler));
        self.inner.on_open(Box::new(move || {
            if let Some(h) = handler.lock().unwrap_or_else(|e| e.into_inner()).take() {
                h();
            }
            Box::pin(async {})
        }));
    }

    /// Registers a handler invoked when a message is received.
    ///
    /// Messages that arrived before the first call are delivered first, in order.
    /// A later call replaces the handler. The handler must not block.
    pub fn on_message(&self, handler: impl Fn(DataChannelMessage) + Send + Sync + 'static) {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_handler(Arc::new(handler));
    }

    /// Registers a handler invoked when the channel closes.
    pub fn on_close(&self, handler: impl Fn() + Send + Sync + 'static) {
        self.inner.on_close(Box::new(move || {
            handler();
            Box::pin(async {})
        }));
    }

    /// Registers a handler invoked on channel errors.
    pub fn on_error(&self, handler: impl Fn(CoreError) + Send + Sync + 'static) {
        self.inner.on_error(Box::new(move |err| {
            handler(CoreError::DataChannel(err.to_string()));
            Box::pin(async {})
        }));
    }

    /// Sets the threshold for [`Self::on_buffered_amount_low`].
    pub fn set_buffered_amount_low_threshold(&self, threshold: usize) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            inner.set_buffered_amount_low_threshold(threshold).await;
        });
    }

    /// Registers a handler invoked when buffered send data drops at or below the low threshold.
    pub fn on_buffered_amount_low(&self, handler: impl Fn() + Send + Sync + 'static) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            inner
                .on_buffered_amount_low(Box::new(move || {
                    handler();
                    Box::pin(async {})
                }))
                .await;
        });
    }

    pub(crate) fn inner(&self) -> Arc<RTCDataChannel> {
        Arc::clone(&self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(n: usize) -> DataChannelMessage {
        DataChannelMessage {
            is_string: true,
            data: Bytes::from(format!("m{n}")),
        }
    }

    fn collector() -> (MessageHandler, Arc<Mutex<Vec<String>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let h: MessageHandler = Arc::new(move |m| {
            g.lock()
                .unwrap()
                .push(String::from_utf8(m.data.to_vec()).unwrap());
        });
        (h, got)
    }

    #[test]
    fn buffered_messages_flush_in_order_then_forward() {
        let mut sink = MessageSink::new();
        for i in 0..3 {
            sink.push("t", msg(i));
        }
        let (h, got) = collector();
        sink.set_handler(h);
        sink.push("t", msg(3));
        assert_eq!(
            *got.lock().unwrap_or_else(|e| e.into_inner()),
            vec!["m0", "m1", "m2", "m3"]
        );
    }

    #[test]
    fn overflow_keeps_newest() {
        let mut sink = MessageSink::new();
        for i in 0..MAX_BUFFERED_MESSAGES + 10 {
            sink.push("t", msg(i));
        }
        let (h, got) = collector();
        sink.set_handler(h);
        let got = got.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(got.len(), MAX_BUFFERED_MESSAGES);
        assert_eq!(got[0], "m10");
        assert_eq!(
            got[MAX_BUFFERED_MESSAGES - 1],
            format!("m{}", MAX_BUFFERED_MESSAGES + 9)
        );
    }
}
