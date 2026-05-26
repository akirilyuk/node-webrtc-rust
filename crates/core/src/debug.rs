//! Optional debug logging controlled by `WEBRTC_DEBUG` or config override.

use std::sync::atomic::{AtomicI8, Ordering};
use std::sync::OnceLock;

const UNSET: i8 = -1;
const FALSE: i8 = 0;
const TRUE: i8 = 1;

static ENV_INIT: OnceLock<bool> = OnceLock::new();
static OVERRIDE: AtomicI8 = AtomicI8::new(UNSET);

fn parse_env(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

fn env_enabled() -> bool {
    *ENV_INIT.get_or_init(|| {
        std::env::var("WEBRTC_DEBUG")
            .map(|value| parse_env(&value))
            .unwrap_or(false)
    })
}

/// Returns whether debug logging is enabled (env var or config override).
pub fn is_debug_enabled() -> bool {
    match OVERRIDE.load(Ordering::Relaxed) {
        UNSET => env_enabled(),
        FALSE => false,
        _ => true,
    }
}

/// Overrides the debug flag from configuration (`RTCConfiguration.debug`).
pub fn set_debug_enabled(enabled: bool) {
    OVERRIDE.store(if enabled { TRUE } else { FALSE }, Ordering::Relaxed);
}

/// Logs a function call when debug mode is enabled.
pub fn debug_fn(module: &str, fn_name: &str, args: &str) {
    if is_debug_enabled() {
        eprintln!("[webrtc-debug] {module}::{fn_name}({args})");
    }
}

/// Logs an event emission when debug mode is enabled.
pub fn debug_event(module: &str, event: &str, detail: &str) {
    if is_debug_enabled() {
        eprintln!("[webrtc-debug] {module} event {event} {detail}");
    }
}

/// Logs a function call when debug mode is enabled; skips argument formatting when disabled.
#[macro_export]
macro_rules! debug_call {
    ($module:expr, $fn_name:expr) => {
        if $crate::debug::is_debug_enabled() {
            $crate::debug::debug_fn($module, $fn_name, "");
        }
    };
    ($module:expr, $fn_name:expr, $($arg:expr),+ $(,)?) => {
        if $crate::debug::is_debug_enabled() {
            $crate::debug::debug_fn($module, $fn_name, &format!($($arg),+));
        }
    };
}

/// Logs an event emission when debug mode is enabled; skips detail formatting when disabled.
#[macro_export]
macro_rules! debug_evt {
    ($module:expr, $event:expr) => {
        if $crate::debug::is_debug_enabled() {
            $crate::debug::debug_event($module, $event, "");
        }
    };
    ($module:expr, $event:expr, $($detail:expr),+ $(,)?) => {
        if $crate::debug::is_debug_enabled() {
            $crate::debug::debug_event($module, $event, &format!($($detail),+));
        }
    };
}
