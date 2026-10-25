// config file parser for the daemon
//
// file format:
//
//   timeout <s> <command...>   -> run <command> after <s> seconds of idle
//   resume  <s> <command...>   -> run <command> when input resumes, for the entry whose
//                                 timeout matches <s>
//
// lines starting with `#` (after optional leading whitespace) are comments and blank lines are
// ignored. <command> is passed verbatim to `sh -c`.
//
// a `timeout` and a `resume` line sharing the same `<s>` value are folded into a single entry,
// since that's exactly how `ext_idle_notifier_v1` models it too: one `ext_idle_notification_v1`
// object per timeout, sending both `idled` and `resumed` events on that same object.
// this is not the same for multiple `timeout` lines sharing the same `<s>`, which get their own
// entries.
//
// command tokens are stored as *offsets* into the raw file buffer, not raw pointers.

use core::ffi::{c_char, c_int};
use core::{array, ptr};

use rustix::fs::{Mode, OFlags, open};
use rustix::io::read;
use shared::exit;
use shared::syscall::{Fork, fork};
use wllib::error::SysResultExt;

use crate::error::AppError;

unsafe extern "C" {
  fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
}

pub const MAX_ENTRIES: usize = 16;
pub const CONFIG_BUF_SIZE: usize = 4096;

#[derive(Clone, Copy, Default)]
pub struct Entry {
  pub timeout_ms: u32,
  pub idle_argv: Option<usize>,
  pub resume_argv: Option<usize>,
}

impl Entry {
  fn new(timeout_ms: u32) -> Self {
    Self {
      timeout_ms,
      ..Default::default()
    }
  }
}

pub struct Config {
  // Reserves its last byte as a guaranteed-zero sentinel (see `read_file`), so the final token
  // in the file is always NUL-terminated even without a trailing newline.
  raw: [u8; CONFIG_BUF_SIZE],
  pub entries: [Entry; MAX_ENTRIES],
  pub entry_len: usize,
}

impl Config {
  // read and parse config file at `path`
  pub fn load(path: &str) -> Result<Self, AppError> {
    let mut raw = [0u8; CONFIG_BUF_SIZE];
    let len = read_file(path, &mut raw)?;

    let mut config = Config {
      raw,
      entries: array::from_fn(|_| Entry::default()),
      entry_len: 0,
    };
    config.parse(len);
    Ok(config)
  }

  // fork and exec the command described by `offsets`. `SIGCHLD` is set to `SIG_IGN` at startup, so
  // the kernel reaps the child automatically.
  pub fn spawn(&self, offset: usize) -> Result<(), AppError> {
    unsafe {
      let fork_res = fork().syserr("fork")?;
      match fork_res {
        Fork::Child => {
          let argv = [
            c"sh".as_ptr(),
            c"-c".as_ptr(),
            self.raw.as_ptr().add(offset).cast(),
            ptr::null(),
          ];
          execvp(argv[0], argv.as_ptr());
          // only reached if execvp fails
          exit::exit(1);
        }
        Fork::Parent(_) => {}
      }
    }
    Ok(())
  }

  fn parse(&mut self, len: usize) {
    let mut i = 0;
    while i < len {
      while i < len && is_space(self.raw[i]) {
        i += 1;
      }
      if i >= len {
        break;
      }
      if self.raw[i] == b'#' {
        i = skip_line(&self.raw, i, len);
        continue;
      }

      // tokenize line in place. whitespace becomes NUL and we record the final token's start
      // offset. slot 0 is the keyword, slot 1 is the timeout, slot 3 is command argv, passed
      // to `sh -c`.
      let mut tok_offsets = [0usize; 3];
      let mut tok_count = 0;
      while i < len && self.raw[i] != b'\n' {
        while i < len && self.raw[i] != b'\n' && is_space(self.raw[i]) {
          // terminate keyword and timeout
          if tok_count < 3 {
            self.raw[i] = 0;
          }
          i += 1;
        }
        if i < len && self.raw[i] != b'\n' && tok_count < 3 {
          tok_offsets[tok_count] = i;
          tok_count += 1;
        }

        if tok_count == 3 {
          while i < len && self.raw[i] != b'\n' {
            i += 1;
          }
        } else {
          while i < len && self.raw[i] != b'\n' && !is_space(self.raw[i]) {
            i += 1;
          }
        }
      }
      if i < len && self.raw[i] == b'\n' {
        // terminate the line's last token too
        self.raw[i] = 0;
        i += 1;
      }
      // not enough tokens for "<keyword> <ms>"
      if tok_count < 3 {
        continue;
      }

      let keyword = match str_at(&self.raw, tok_offsets[0]) {
        b"timeout" => Some(true),
        b"resume" => Some(false),
        _ => None,
      };
      let Some(is_timeout) = keyword else {
        continue;
      };
      let Some(timeout_s) = parse_decimal(str_at(&self.raw, tok_offsets[1])) else {
        continue;
      };
      // a *successfully parsed* value of 0 is syntactically fine but not a usable timeout, so it's
      // worth complaining about.
      if timeout_s == 0 {
        AppError::InvalidTimeout.write_diagnostic();
        continue;
      }
      let cmd_offset = tok_offsets[2];

      let Some(entry) = self.create(timeout_s * 1000, is_timeout) else {
        continue;
      };
      if is_timeout {
        entry.idle_argv = Some(cmd_offset);
      } else {
        entry.resume_argv = Some(cmd_offset);
      }
    }
  }

  fn create(&mut self, timeout_ms: u32, is_timeout: bool) -> Option<&mut Entry> {
    // find matching entries for the given timeout, but only if it's a resume entry. prevents
    // rolling multiple timeout entries with the same timeout into one.
    if let Some(idx) = self.entries[..self.entry_len]
      .iter()
      .position(|e| e.timeout_ms == timeout_ms)
      && !is_timeout
    {
      return Some(&mut self.entries[idx]);
    }

    if self.entry_len >= MAX_ENTRIES {
      return None;
    }

    self.entries[self.entry_len] = Entry::new(timeout_ms);
    self.entry_len += 1;
    Some(&mut self.entries[self.entry_len - 1])
  }
}

fn is_space(b: u8) -> bool {
  b == b' ' || b == b'\t' || b == b'\r'
}

fn skip_line(buf: &[u8], mut i: usize, len: usize) -> usize {
  while i < len && buf[i] != b'\n' {
    i += 1;
  }
  if i < len {
    i += 1;
  }
  i
}

fn str_at(buf: &[u8], offset: usize) -> &[u8] {
  let mut end = offset;
  while end < buf.len() && buf[end] != 0 {
    end += 1;
  }
  &buf[offset..end]
}

fn parse_decimal(bytes: &[u8]) -> Option<u32> {
  if bytes.is_empty() {
    return None;
  }
  let mut val: u32 = 0;
  for &b in bytes {
    if !b.is_ascii_digit() {
      return None;
    }
    val = val.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
  }
  Some(val)
}

fn read_file(path: &str, buf: &mut [u8]) -> Result<usize, AppError> {
  let fd = open(path, OFlags::RDONLY, Mode::empty()).syserr("open")?;
  let mut offset = 0usize;
  // Reserve the last byte of `buf` as a guaranteed-zero sentinel, so the final token in the
  // file is NUL-terminated even if the file has no trailing newline.
  let read_cap = buf.len() - 1;
  while offset < read_cap {
    let remaining = read(&fd, &mut buf[offset..read_cap]).syserr("read")?;
    if remaining == 0 {
      break;
    }
    offset += remaining;
  }
  Ok(offset)
}
