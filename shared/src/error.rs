//! Diagnostic logging and error rendering for applications.

use rustix::fd::OwnedFd;
use wllib::error::{SysError, WireError};
use wllib::fmt_lite::StringOnStack;
use wllib::transport::Connection;
use wllib::wire::Message;

use crate::io::write_stderr;

/// Write a wire error to stderr tagged with `prefix`.
pub fn write_wire_diagnostic(prefix: &str, err: &WireError) {
  match err {
    WireError::Sys(e) => write_sys_diagnostic(prefix, e),
    WireError::Protocol(p) => {
      let mut s = StringOnStack::<128>::new();
      s.push(prefix)
        .push(" wayland protocol error: object ")
        .push(p.object_id)
        .push(", code ")
        .push(p.code)
        .push(", message: ")
        .push(p.message.as_str())
        .push("\n");
      write_stderr(s);
    }
    WireError::InvalidPath => {
      let mut s = StringOnStack::<128>::new();
      s.push(prefix)
        .push(" WAYLAND_DISPLAY or XDG_RUNTIME_DIR not set or socket path invalid\n");
      write_stderr(s);
    }
    WireError::BufferTooSmall => {
      let mut s = StringOnStack::<128>::new();
      s.push(prefix)
        .push(" receive buffer too small for wayland framing\n");
      write_stderr(s);
    }
    WireError::ConnectionClosed => {
      let mut s = StringOnStack::<128>::new();
      s.push(prefix).push(" connection closed by compositor\n");
      write_stderr(s);
    }
  }
}

/// Write a syscall error to stderr tagged with `prefix`.
pub fn write_sys_diagnostic(prefix: &str, err: &SysError) {
  let mut s = StringOnStack::<128>::new();
  s.push(prefix)
    .push(" syscall failed: ")
    .push(err.call)
    .push(" (")
    .push(err.errno)
    .push(")\n");
  write_stderr(s);
}

/// Extension trait on [`Connection`] for sending error logged messages.
pub trait ConnectionExt {
  fn send_logged(&self, msg: &Message, fd: Option<&OwnedFd>);
}

impl ConnectionExt for Connection {
  fn send_logged(&self, msg: &Message, fd: Option<&OwnedFd>) {
    if let Err(e) = self.send(msg, fd) {
      write_wire_diagnostic("wllib", &e);
    }
  }
}
