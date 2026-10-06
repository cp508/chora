//! chora — the library of commands.
//!
//! The binary in `main.rs` is a thin dispatcher (`proj-lib-main-split`):
//! all logic lives here, so it can be tested.

pub mod cli;
pub mod config;
pub mod editor;
pub mod error;
pub mod event;
pub mod git;
pub mod log;
pub mod output;
pub mod store;
pub mod tui;
