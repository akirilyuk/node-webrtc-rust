//! Tokio runtime initialization for async NAPI methods.

use napi::bindgen_prelude::create_custom_tokio_runtime;
use tokio::runtime::Builder;

#[napi::module_init]
fn init() {
    let rt = Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("failed to create Tokio runtime");
    create_custom_tokio_runtime(rt);
}
