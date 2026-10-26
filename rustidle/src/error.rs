use shared::error::{write_sys_diagnostic, write_wire_diagnostic};
use shared::io::write_stderr;
use wllib::error::{SysError, WireError};

pub enum AppError {
  // A failure from the wayland transport/protocol layer.
  Wire(WireError),
  // A syscall outside the wayland socket failed (memfd_create, mmap, pipe2, fork, ...).
  Sys(SysError),
  // A `timeout <s> ...` config line parsed a value of 0 seconds.
  InvalidTimeout,
}

impl AppError {
  pub fn write_diagnostic(&self) {
    match self {
      AppError::Wire(e) => write_wire_diagnostic("[rustidle]", e),
      AppError::Sys(e) => write_sys_diagnostic("[rustidle]", e),
      AppError::InvalidTimeout => write_stderr("Timeout must be at least 1s\n"),
    }
  }
}

impl From<WireError> for AppError {
  fn from(e: WireError) -> Self {
    AppError::Wire(e)
  }
}
