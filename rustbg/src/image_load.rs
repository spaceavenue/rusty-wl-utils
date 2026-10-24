use rustix::fs::{Mode, OFlags, open};
use rustix::io::read;
use rustix::pipe::{PipeFlags, pipe_with};
use rustix::stdio;
use wllib::error::{SysError, SysResultExt};
use wllib::fmt_lite::StringOnStack;

use crate::error::AppError;
use crate::state::Config;

// this was a bit hard to follow so im documenting for my own reference
// we:
// 1. create a unix pipe
// 2. spawn the dump-bgra child process (fork + execvp)
// 3. connect it's write end (stdout) to the write end of the pipe
// 4. then read the raw bgra pixels from our read end, directly into the mmap_slice
fn exec_and_read<const N: usize>(
  argv: [*const libc::c_char; N],
  buffer: &mut [u8],
) -> Result<(), AppError> {
  // create an unidirectional pipe to read data from child process
  // O_CLOEXEC closes the read/write ends in any other spawned children
  let (read_fd, write_fd) = pipe_with(PipeFlags::CLOEXEC).syserr("pipe")?;

  let pid = unsafe { libc::fork() };
  if pid < 0 {
    return Err(AppError::Sys(SysError::last("fork")));
  }
  // child process context
  // 1. redirect stdout to the write end of the pipe
  // 2. restore default SIGPIPE handler before exec
  if pid == 0 {
    drop(read_fd);

    let _ = stdio::dup2_stdout(&write_fd);
    drop(write_fd);

    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    // redirect stderr to /dev/null
    if let Ok(devnull) = open(c"/dev/null", OFlags::WRONLY, Mode::empty()) {
      let _ = stdio::dup2_stderr(&devnull);
      drop(devnull);
    }

    unsafe { libc::execvp(argv[0], argv.as_ptr()) };
    // if execvp failed, kill child >:)
    unsafe { libc::_exit(1) };
  }

  // parent process context
  // 1. close the write end in parent
  // 2. read raw bytes from the pipe directly into buffer
  drop(write_fd);

  let mut offset = 0usize;
  while offset < buffer.len() {
    let remaining = read(&read_fd, &mut buffer[offset..]).syserr("read")?;
    if remaining == 0 {
      break;
    }
    offset += remaining;
  }

  drop(read_fd);
  unsafe { libc::waitpid(pid, core::ptr::null_mut(), 0) };

  if offset != buffer.len() {
    return Err(AppError::ImageDecodeError);
  }
  Ok(())
}

#[cfg(feature = "ffmpeg")]
fn run_ffmpeg(
  out_width: u32,
  out_height: u32,
  buffer: &mut [u8],
  fill: bool,
  path: StringOnStack<256>,
) -> Result<(), AppError> {
  let mut filter = StringOnStack::<96>::new();

  filter
    .push("scale=")
    .push(out_width)
    .push(":")
    .push(out_height);

  match fill {
    true => {
      filter
        .push(":force_original_aspect_ratio=increase,crop=")
        .push(out_width)
        .push(":")
        .push(out_height);
    }
    false => {
      filter
        .push(":force_original_aspect_ratio=decrease,pad=")
        .push(out_width)
        .push(":")
        .push(out_height)
        .push(":(ow-iw)/2:(oh-ih)/2");
    }
  }

  // build ffmpeg argument vector
  let argv: [*const libc::c_char; 11] = [
    c"ffmpeg".as_ptr(),
    c"-i".as_ptr(),
    path.as_ptr(),
    c"-vf".as_ptr(),
    filter.as_ptr(),
    c"-f".as_ptr(),
    c"rawvideo".as_ptr(),
    c"-pix_fmt".as_ptr(),
    c"bgra".as_ptr(),
    c"-".as_ptr(),
    core::ptr::null(),
  ];
  exec_and_read(argv, buffer)
}

#[cfg(not(feature = "ffmpeg"))]
fn run_dump_bgra(
  out_width: u32,
  out_height: u32,
  buffer: &mut [u8],
  fill: bool,
  path: StringOnStack<256>,
) -> Result<(), AppError> {
  let mut dim_str = StringOnStack::<21>::new();
  dim_str.push(out_width).push("x").push(out_height);
  let mut mode_str = StringOnStack::<5>::new();
  if fill {
    mode_str.push("fill");
  } else {
    mode_str.push("fit");
  }

  // build dump-bgra argument vector: scale image to raw bgra pixels and stream to stdout
  let argv: [*const libc::c_char; 6] = [
    c"dump-bgra".as_ptr(),
    dim_str.as_ptr(),
    mode_str.as_ptr(),
    path.as_ptr(),
    c"-".as_ptr(),
    core::ptr::null(),
  ];
  exec_and_read(argv, buffer)
}

pub fn load_and_scale(
  out_width: u32,
  out_height: u32,
  buffer: &mut [u8],
  config: &Config,
) -> Result<(), AppError> {
  let path = config.image_path;
  let fill = config.fill;

  #[cfg(not(feature = "ffmpeg"))]
  {
    run_dump_bgra(out_width, out_height, buffer, fill, path)
  }

  #[cfg(feature = "ffmpeg")]
  {
    run_ffmpeg(out_width, out_height, buffer, fill, path)
  }
}
