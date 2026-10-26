//! Shared utilities for rusty-wl-utils.

#![no_std]

pub mod cli;
pub mod env;
pub mod error;
pub mod io;
pub mod syscall;

pub use env::connect_wayland;
pub use error::{ConnectionExt, write_sys_diagnostic, write_wire_diagnostic};
pub use syscall::{execvp, exit, fork, signal};
