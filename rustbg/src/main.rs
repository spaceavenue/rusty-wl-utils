#![no_std]
#![no_main]

pub mod error;
pub mod image_load;
pub mod shm;
pub mod state;

use core::ffi::{c_char, c_int};

use rustix::fs::{Access, access};
use shared::cli::{Arg, ArgParser, get_args_iter};
use shared::error::{ConnectionExt, write_wire_diagnostic};
use shared::io::write_stderr;
use wllib::dispatch::dispatch_once;
use wllib::error::WireError::ConnectionClosed;
use wllib::protocols::{wl_compositor, wl_surface, zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wllib::registry::crawl;
use wllib::wire::Message;

use crate::state::{Config, State};

#[link(name = "c", kind = "static")]
unsafe extern "C" {}

fn write_usage() -> c_int {
  write_stderr("Usage: rustbg [-f | --fill] [-n | --namespace <name>] <image path>\n");
  1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: isize, argv: *const *mut c_char) -> c_int {
  let mut config = Config::default();

  if argc <= 0 || argv.is_null() {
    return 1;
  }

  let args_iter = unsafe { get_args_iter(argc as usize, argv) };

  let mut parser = ArgParser::new(args_iter);
  while let Some(arg) = parser.next_arg() {
    match arg {
      Arg::Short('f') | Arg::Long("fill") => config.fill = true,
      Arg::Short('n') | Arg::Long("namespace") => {
        let Some(namespace) = parser.next_val() else {
          write_stderr("[rustbg] no namespace provided.");
          return write_usage();
        };
        config.namespace.clear();
        config.namespace.push(namespace);
      }
      Arg::Value(path) => {
        config.image_path.push(path);
      }
      Arg::Short(_) | Arg::Long(_) => return write_usage(),
    }
  }

  if config.image_path.is_empty() {
    write_stderr("[rustbg] no image path supplied\n");
    return write_usage();
  }

  if access(config.image_path.as_c_str(), Access::EXISTS).is_err() {
    write_stderr("[rustbg] cannot open image file.\n");
    return write_usage();
  }

  let mut conn = match shared::connect_wayland() {
    Ok(c) => c,
    Err(e) => {
      write_wire_diagnostic("[rustbg]", &e);
      return 1;
    }
  };

  let mut state = State::init(config);

  if let Err(e) = crawl(&mut conn, &mut state) {
    write_wire_diagnostic("[rustbg]", &e);
    return 1;
  }

  // setup layer surfaces and gamma control for all monitors.
  for i in 0..state.output_len {
    // alloc ids for the new wl_surface (from compositor) and layer surface.
    let surf_id = conn.alloc_id();
    let layer_surf_id = conn.alloc_id();
    let output_id = state.outputs[i].output_id;

    // this is a multi step process
    //
    // 1. creates a wl_surface
    let mut surf_msg = Message::new(
      state.global.compositor_id,
      wl_compositor::request::CREATE_SURFACE,
    );
    surf_msg.write_u32(surf_id);
    conn.send_logged(&surf_msg, None);

    // 2. wrap the wl_surface as a layer surface
    let mut ls_msg = Message::new(
      state.global.layer_shell_id,
      zwlr_layer_shell_v1::request::GET_LAYER_SURFACE,
    );
    ls_msg.write_u32(layer_surf_id);
    ls_msg.write_u32(surf_id);
    ls_msg.write_u32(output_id);
    ls_msg.write_u32(zwlr_layer_shell_v1::layer::BACKGROUND);
    ls_msg.write_cstr(state.config.namespace.as_c_str());
    conn.send_logged(&ls_msg, None);

    // anchor to all edges for full screen
    let mut anchor_msg = Message::new(layer_surf_id, zwlr_layer_surface_v1::request::SET_ANCHOR);
    anchor_msg.write_u32(zwlr_layer_surface_v1::anchor::ALL);
    conn.send_logged(&anchor_msg, None);

    // configure exclusive zone to go behind other layer surfaces, like panels or status bars
    let mut ex_msg = Message::new(
      layer_surf_id,
      zwlr_layer_surface_v1::request::SET_EXCLUSIVE_ZONE,
    );
    // -1 -> do not request any exclusive zone.
    ex_msg.write_i32(-1);
    conn.send_logged(&ex_msg, None);

    // initial attach
    let mut attach_msg = Message::new(surf_id, wl_surface::request::ATTACH);
    // NULL -> map surface
    attach_msg.write_u32(0);
    attach_msg.write_i32(0);
    attach_msg.write_i32(0);
    conn.send_logged(&attach_msg, None);
    // commit initial surface
    conn.send_logged(&Message::new(surf_id, wl_surface::request::COMMIT), None);

    // store the allocated ids back into our Output instance
    state.outputs[i].wl_surface_id = surf_id;
    state.outputs[i].layer_surface_id = layer_surf_id;
  }

  // main wayland event dispatch loop
  loop {
    match dispatch_once(&mut conn, &mut state) {
      Ok(()) => (),
      Err(ConnectionClosed) => break,
      Err(e) => {
        write_wire_diagnostic("[rustbg]", &e);
        break;
      }
    }
  }
  0
}
