//! Helpers for wiring conference mpsc channels to NAPI ThreadsafeFunctions.

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ThreadsafeFunction, ThreadsafeFunctionCallMode, ThreadSafeCallContext,
};
use napi::JsFunction;
use tokio::sync::mpsc::UnboundedReceiver;

/// Spawns a Tokio task that reads from an mpsc channel and invokes a ThreadsafeFunction.
pub fn wire_event_channel<T>(mut rx: UnboundedReceiver<T>, tsfn: ThreadsafeFunction<T>)
where
    T: Send + 'static,
{
    spawn(async move {
        while let Some(value) = rx.recv().await {
            let _ = tsfn.call(Ok(value), ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}

/// Creates a ThreadsafeFunction that forwards mapped values to a JS callback.
pub fn create_event_callback<T, V, F>(
    env: &Env,
    callback: JsFunction,
    callback_fn: F,
) -> Result<ThreadsafeFunction<T>>
where
    T: Send + 'static,
    V: ToNapiValue,
    F: 'static + Send + FnMut(ThreadSafeCallContext<T>) -> Result<Vec<V>>,
{
    env.create_threadsafe_function(&callback, 0, callback_fn)
}
