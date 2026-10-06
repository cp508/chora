//! Leveled logging. Default is `Warn`; `--debug` enables everything.

use std::fmt::Arguments;
use std::sync::atomic::{AtomicU8, Ordering};

/// Log levels, least to most verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 1,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Warn as u8);

/// `true` (from `--debug`) enables everything.
pub fn init(debug: bool) {
    LEVEL.store(
        if debug { Level::Debug } else { Level::Warn } as u8,
        Ordering::Relaxed,
    );
}

fn enabled(level: Level) -> bool {
    (level as u8) <= LEVEL.load(Ordering::Relaxed)
}

/// One stderr line when `level` is enabled.
pub fn log(level: Level, args: Arguments<'_>) {
    if enabled(level) {
        eprintln!("[{}] {args}", level.name());
    }
}

/// Visible with `--debug`.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => { $crate::log::log($crate::log::Level::Debug, format_args!($($arg)*)) };
}

/// Visible with `--debug`; the CLI stays quiet by default.
#[macro_export]
macro_rules! info_log {
    ($($arg:tt)*) => { $crate::log::log($crate::log::Level::Info, format_args!($($arg)*)) };
}

/// Visible by default.
#[macro_export]
macro_rules! warn_log {
    ($($arg:tt)*) => { $crate::log::log($crate::log::Level::Warn, format_args!($($arg)*)) };
}
