use shared::error::{write_sys_diagnostic, write_wire_diagnostic};
use shared::io;
use wllib::error::{SysError, WireError};

// Application-level errors.
//
// Wraps [`wllib::WireError`] for anything related to the wayland connection/protocol. Each
// variant carries enough context to render a real diagnostic via `write_diagnostic`.
pub enum AppError {
  // A failure from the wayland transport/protocol layer.
  Wire(WireError),
  // A syscall outside the wayland socket failed.
  Sys(SysError),
  // dump-bgra/ffmpeg produced fewer bytes than the expected `width * height * 4`.
  ImageDecodeError,
}

impl AppError {
  pub fn write_diagnostic(&self) {
    match self {
      AppError::Wire(e) => write_wire_diagnostic("[rustbg]", e),
      AppError::Sys(e) => write_sys_diagnostic("[rustbg]", e),
      #[cfg(feature = "ffmpeg")]
      AppError::ImageDecodeError => {
        io::write_stderr("[rustbg] image decode error: ffmpeg produced fewer bytes than expected\n")
      }
      #[cfg(not(feature = "ffmpeg"))]
      AppError::ImageDecodeError => io::write_stderr(
        "[rustbg] image decode error: dump-bgra produced fewer bytes than expected\n",
      ),
    }
  }
}

impl From<WireError> for AppError {
  fn from(e: WireError) -> Self {
    AppError::Wire(e)
  }
}
