//! Helpers for wiring core mpsc channels to NAPI ThreadsafeFunctions.

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ThreadsafeFunction, ThreadsafeFunctionCallMode, ThreadSafeCallContext,
};
use napi::JsFunction;
use napi::JsUnknown;
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

/// Creates a ThreadsafeFunction for callbacks with no arguments.
pub fn create_void_callback(
    env: &Env,
    callback: JsFunction,
) -> Result<ThreadsafeFunction<()>> {
    env.create_threadsafe_function(
        &callback,
        0,
        |_ctx: ThreadSafeCallContext<()>| -> Result<Vec<JsUnknown>> { Ok(vec![]) },
    )
}

/// Creates a ThreadsafeFunction for error-only callbacks.
pub fn create_error_callback(
    env: &Env,
    callback: JsFunction,
) -> Result<ThreadsafeFunction<String>> {
    env.create_threadsafe_function(&callback, 0, |ctx: ThreadSafeCallContext<String>| {
        Ok(vec![ctx.env.create_string(ctx.value.as_str())?])
    })
}

/// Spawns a Tokio task for void ThreadsafeFunction invocations.
pub fn wire_void_channel(mut rx: UnboundedReceiver<()>, tsfn: ThreadsafeFunction<()>) {
    spawn(async move {
        while rx.recv().await.is_some() {
            let _ = tsfn.call(Ok(()), ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}

/// Spawns a Tokio task for error ThreadsafeFunction invocations.
pub fn wire_error_channel(mut rx: UnboundedReceiver<String>, tsfn: ThreadsafeFunction<String>) {
    spawn(async move {
        while let Some(message) = rx.recv().await {
            let _ = tsfn.call(Ok(message), ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}
