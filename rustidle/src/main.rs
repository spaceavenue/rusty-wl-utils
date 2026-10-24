#![no_std]
#![no_main]

pub mod config;
pub mod error;
pub mod state;

use core::ffi::{c_char, c_int};

use rustix::fs::{Access, access};
use wllib::dispatch::dispatch_once;
use wllib::error::WireError::ConnectionClosed;
use wllib::io::write_stderr;
use wllib::protocols::ext_idle_notifier_v1;
use wllib::registry::crawl;
use wllib::transport::Connection;
use wllib::wire::Message;

use crate::config::Config;
use crate::state::State;

const SIGCHLD: c_int = 17;
const SIGUSR1: c_int = 10;
const SIG_IGN: usize = 1;

#[repr(C)]
struct SigAction {
  sa_sigaction: usize,
  sa_mask: [u64; 16],
  sa_flags: c_int,
  sa_restorer: Option<unsafe extern "C" fn()>,
}

unsafe extern "C" {
  fn signal(sig: c_int, handler: usize) -> usize;
  fn sigaction(sig: c_int, act: *const SigAction, oact: *mut SigAction) -> c_int;
  fn sigemptyset(set: *mut [u64; 16]) -> c_int;
}

#[link(name = "c", kind = "static")]
unsafe extern "C" {}

// flag to track the suspended state
static IS_SUSPENDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

extern "C" fn handle_sigusr1(_sig: c_int) {
  // fetch_xor with true inverts the current boolean value
  IS_SUSPENDED.fetch_xor(true, core::sync::atomic::Ordering::Relaxed);
}

fn write_usage() -> c_int {
  write_stderr("Usage: rustidle <config-file>\n");
  1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: isize, argv: *const *mut c_char) -> c_int {
  if argc != 2 {
    return write_usage();
  }
  let config_path = unsafe { *argv.add(1) };
  if config_path.is_null() {
    return write_usage();
  }
  let config_path = unsafe { core::ffi::CStr::from_ptr(config_path) };
  if access(config_path, Access::EXISTS).is_err() {
    write_stderr("[rustidle] failed to open config file.\n");
    return 1;
  }

  // tells kernel to reap child processes automatically, avoiding waitpid
  unsafe { signal(SIGCHLD, SIG_IGN) };
  // handle SIGUSR1
  unsafe {
    let mut sa: SigAction = core::mem::zeroed();
    sa.sa_sigaction = handle_sigusr1 as *const () as usize;
    // sa_flags = 0 so that blocking syscalls (recv, etc.) fail with EINTR instead of restarting
    sa.sa_flags = 0;
    sigemptyset(&raw mut sa.sa_mask);
    sigaction(SIGUSR1, &raw const sa, core::ptr::null_mut());
  }

  let mut conn = match Connection::connect() {
    Ok(c) => c,
    Err(e) => {
      e.write_diagnostic();
      return 1;
    }
  };
  let config = match Config::load(config_path) {
    Ok(c) => c,
    Err(e) => {
      e.write_diagnostic();
      return 1;
    }
  };
  let mut state = State::init(config);

  if let Err(e) = crawl(&mut conn, &mut state) {
    e.write_diagnostic();
    return 1;
  }

  let create_notifs = |conn: &mut Connection, state: &mut State| {
    for i in 0..state.config.entry_len {
      let notif = &mut state.notifications[i];
      notif.id = conn.alloc_id();
      let mut msg = Message::new(
        state.global.idle_notifier_id,
        ext_idle_notifier_v1::request::GET_IDLE_NOTIFICATION,
      );
      msg.write_u32(notif.id);
      msg.write_u32(notif.entry.timeout_ms);
      msg.write_u32(state.global.seat_id);

      conn.send_logged(&msg, None);
    }
  };

  create_notifs(&mut conn, &mut state);
  let mut was_suspended = false;

  // main wayland event dispatch loop
  loop {
    if IS_SUSPENDED.load(core::sync::atomic::Ordering::Relaxed) {
      for i in 0..state.config.entry_len {
        let notif = &mut state.notifications[i];
        let msg = Message::new(notif.id, ext_idle_notifier_v1::request::DESTROY);
        conn.send_logged(&msg, None);
      }
      was_suspended = true;
    }

    while IS_SUSPENDED.load(core::sync::atomic::Ordering::Relaxed) {
      rustix::event::pause();
    }

    if was_suspended {
      create_notifs(&mut conn, &mut state);
      was_suspended = false;
    }

    match dispatch_once(&mut conn, &mut state) {
      Ok(()) => (),
      Err(ConnectionClosed) => break,
      Err(e) => {
        if IS_SUSPENDED.load(core::sync::atomic::Ordering::Relaxed) {
          continue;
        }
        e.write_diagnostic();
        break;
      }
    }
  }
  0
}
