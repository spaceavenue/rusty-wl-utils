//! Shared utilities for rusty-wl-utils.

#![no_std]

pub mod cli;
pub mod syscall;

pub use syscall::{exit, fork, signal};
