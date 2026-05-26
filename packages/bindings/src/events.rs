//! Helpers for wiring core mpsc channels to NAPI ThreadsafeFunctions.

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue};
use tokio::sync::mpsc::UnboundedReceiver;

/// Spawns a Tokio task that reads from an mpsc channel and invokes a ThreadsafeFunction.
pub fn wire_event_channel<T>(
    mut rx: UnboundedReceiver<T>,
    tsfn: ThreadsafeFunction<T, UnknownReturnValue, T, Status, false>,
) where
    T: Send + 'static,
{
    tokio::spawn(async move {
        while let Some(value) = rx.recv().await {
            let _ = tsfn.call(value, ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}

/// Creates a ThreadsafeFunction that forwards a single argument to a JS callback.
pub fn create_event_callback<T, F>(
    env: &Env,
    callback: Function<F, UnknownReturnValue>,
    map: fn(&Env, T) -> Result<Vec<Unknown<'static>>>,
) -> Result<ThreadsafeFunction<T, UnknownReturnValue, T, Status, false>>
where
    T: Send + 'static,
    F: for<'env> Fn(&'env Env, Unknown<'env>) -> Result<UnknownReturnValue> + 'static,
{
    env.create_threadsafe_function(&callback, 0, move |ctx| map(ctx.env, ctx.value))
}

/// Creates a ThreadsafeFunction for callbacks with no arguments.
pub fn create_void_callback<F>(
    env: &Env,
    callback: Function<F, UnknownReturnValue>,
) -> Result<ThreadsafeFunction<(), UnknownReturnValue, (), Status, false>>
where
    F: for<'env> Fn(&'env Env, Unknown<'env>) -> Result<UnknownReturnValue> + 'static,
{
    env.create_threadsafe_function(&callback, 0, |_ctx| Ok(vec![]))
}

/// Creates a ThreadsafeFunction for error-only callbacks.
pub fn create_error_callback<F>(
    env: &Env,
    callback: Function<F, UnknownReturnValue>,
) -> Result<ThreadsafeFunction<String, UnknownReturnValue, String, Status, false>>
where
    F: for<'env> Fn(&'env Env, Unknown<'env>) -> Result<UnknownReturnValue> + 'static,
{
    env.create_threadsafe_function(&callback, 0, |ctx| {
        let message = ctx.env.create_string(&ctx.value)?;
        Ok(vec![message.into()])
    })
}

/// Spawns a Tokio task for void ThreadsafeFunction invocations.
pub fn wire_void_channel(
    mut rx: UnboundedReceiver<()>,
    tsfn: ThreadsafeFunction<(), UnknownReturnValue, (), Status, false>,
) {
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            let _ = tsfn.call((), ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}

/// Spawns a Tokio task for error ThreadsafeFunction invocations.
pub fn wire_error_channel(
    mut rx: UnboundedReceiver<String>,
    tsfn: ThreadsafeFunction<String, UnknownReturnValue, String, Status, false>,
) {
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let _ = tsfn.call(message, ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}
