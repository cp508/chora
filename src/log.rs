//! Debug logging: quiet by default, `--debug` prints to stderr.

use std::fmt::Arguments;
use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG: AtomicBool = AtomicBool::new(false);

/// `true` (from `--debug`) enables debug output on stderr.
pub fn init(debug: bool) {
    DEBUG.store(debug, Ordering::Relaxed);
}

/// One stderr line when `--debug` is enabled.
pub fn debug(args: Arguments<'_>) {
    if DEBUG.load(Ordering::Relaxed) {
        eprintln!("[debug] {args}");
    }
}

/// Visible with `--debug`.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => { $crate::log::debug(format_args!($($arg)*)) };
}
