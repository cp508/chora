//! chora library. `main.rs` is a thin dispatcher.

pub mod calendar;
#[cfg(feature = "cli")]
pub mod cli;
pub mod config;
#[cfg(feature = "cli")]
pub mod editor;
pub mod error;
pub mod event;
#[cfg(feature = "cli")]
pub mod git;
pub mod index;
pub mod log;
pub mod note;
pub mod output;
pub mod store;
#[cfg(feature = "cli")]
pub mod sync;
#[cfg(feature = "cli")]
pub mod tui;
