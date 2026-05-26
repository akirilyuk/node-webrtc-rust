//! Tokio runtime initialization for async NAPI methods.

#[napi::module_init]
fn init() {
    napi::bindgen_prelude::create_custom_tokio_runtime(false, 4, true);
}
