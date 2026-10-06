//! Minimal leveled logging for developers.
//!
//! Default level is `Warn` (quiet CLI, errors still surface via `anyhow`).
//! The global `--debug` flag switches to `Debug`.

use std::fmt::Arguments;
use std::sync::atomic::{AtomicU8, Ordering};

/// Log levels, ordered by verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Failures (also returned to the shell via `anyhow`).
    Error = 1,
    /// Suspicious states worth a look.
    Warn,
    /// Milestones of what the program did.
    Info,
    /// Internals: commands run, paths resolved, state transitions.
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

/// Select the level: `true` (from `--debug`) means everything.
pub fn init(debug: bool) {
    LEVEL.store(
        if debug { Level::Debug } else { Level::Warn } as u8,
        Ordering::Relaxed,
    );
}

fn enabled(level: Level) -> bool {
    (level as u8) <= LEVEL.load(Ordering::Relaxed)
}

/// Emit one line on stderr when `level` is enabled.
pub fn log(level: Level, args: Arguments<'_>) {
    if enabled(level) {
        eprintln!("[{}] {args}", level.name());
    }
}

/// Debug line (visible with `--debug`).
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => { $crate::log::log($crate::log::Level::Debug, format_args!($($arg)*)) };
}

/// Info line (visible with `--debug`; the CLI stays quiet by default).
#[macro_export]
macro_rules! info_log {
    ($($arg:tt)*) => { $crate::log::log($crate::log::Level::Info, format_args!($($arg)*)) };
}

/// Warning line (visible by default).
#[macro_export]
macro_rules! warn_log {
    ($($arg:tt)*) => { $crate::log::log($crate::log::Level::Warn, format_args!($($arg)*)) };
}
