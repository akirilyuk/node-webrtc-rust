//! One process-wide ONNX Runtime API for every `ort` user in the process.
//!
//! With `ort-shared-backend`, `ort` is built with `alternative-backend` (no linking of its own), and
//! the API comes from the ONNX Runtime that sherpa-onnx links into the binary (`OrtGetApiBase`). This
//! is the only place that calls [`ort::set_api`]; every `ort` session (Whisper LID scores, Silero
//! VAD) calls [`ensure_ort_api`] first.

/// Installs the linked ONNX Runtime's API into `ort` once. Idempotent and thread-safe.
#[cfg(feature = "ort-shared-backend")]
pub fn ensure_ort_api() -> Result<(), String> {
    use std::sync::OnceLock;
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        // SAFETY: `OrtGetApiBase` is the ONNX Runtime C entry point; it returns a static table.
        let base = unsafe { ort::sys::OrtGetApiBase() };
        if base.is_null() {
            return Err("ONNX Runtime OrtGetApiBase returned null".into());
        }
        // SAFETY: `base` is non-null and points at the runtime's static `OrtApiBase`.
        let api = unsafe { ((*base).GetApi)(ort::sys::ORT_API_VERSION) };
        if api.is_null() {
            return Err(format!(
                "linked ONNX Runtime does not serve C API version {}",
                ort::sys::ORT_API_VERSION
            ));
        }
        // SAFETY: `api` is non-null and points at the runtime's static `OrtApi` table.
        // `set_api` returns false only when an API is already installed, which is fine.
        let _ = ort::set_api(unsafe { std::ptr::read(api) });
        Ok(())
    })
    .clone()
}

/// Without the shared backend, `ort` links its own runtime and needs no setup.
#[cfg(not(feature = "ort-shared-backend"))]
pub fn ensure_ort_api() -> Result<(), String> {
    Ok(())
}
