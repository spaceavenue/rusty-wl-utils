#![no_std]
#![no_main]

mod error;
mod gamma;
mod state;

use core::ffi::{c_char, c_int};

use shared::cli::{Arg, ArgParser, get_args_iter};
use shared::error::{ConnectionExt, write_wire_diagnostic};
use shared::io::{write_stderr, write_stdout};
use state::{Config, State};
use wllib::dispatch::dispatch_once;
use wllib::error::WireError::ConnectionClosed;
use wllib::protocols::zwlr_gamma_control_manager_v1;
use wllib::registry::crawl;
use wllib::wire::Message;


const USAGE: &str = concat!(
  "Usage: rustemp -t <kelvin>\n",
  "              [--black <0..1>] [--white <0..1>]\n",
  "              [-g|--gamma <exponent>]\n",
  "              [--gain <r>:<g>:<b>] [--offset <r>:<g>:<b>]\n",
  "              [-c|--contrast <-1..1>] [-b|--brightness <multiplier>]\n",
  "              [-p|--posterize <levels>]\n",
  "              [--solarize[=<0..1>]] [-i|--invert]\n",
  "              [--no-red] [--no-green] [--no-blue]\n",
);

fn bad_value() -> ! {
  shared::exit::die("rustemp: invalid or no value supplied\n")
}

fn parse_f64(s: &str) -> Option<f64> {
  s.parse::<f64>().ok()
}

// parses a "<r>:<g>:<b>" string, e.g. a --gain/--offset argument.
fn parse_triplet(s: &str) -> Option<(f64, f64, f64)> {
  let mut parts = s.splitn(3, ':');
  let r = parts.next()?.parse::<f64>().ok()?;
  let g = parts.next()?.parse::<f64>().ok()?;
  let b = parts.next()?.parse::<f64>().ok()?;
  Some((r, g, b))
}

fn parse_u32(s: &str) -> Option<u32> {
  s.parse::<u32>().ok()
}

fn parse_args<'a>(args_iter: impl Iterator<Item = &'a str>, config: &mut Config) -> Option<()> {
  let mut parser = ArgParser::new(args_iter);
  while let Some(arg) = parser.next_arg() {
    match arg {
      Arg::Short('t') | Arg::Long("temp") => {
        config.temp = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('k') | Arg::Long("black") => {
        config.level_black = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('w') | Arg::Long("white") => {
        config.level_white = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('g') | Arg::Long("gamma") => {
        config.gamma = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('G') | Arg::Long("gain") => {
        let (r, g, b) = parse_triplet(parser.next_val()?)?;
        (config.gain_r, config.gain_g, config.gain_b) = (r, g, b);
      }
      Arg::Short('O') | Arg::Long("offset") => {
        let (r, g, b) = parse_triplet(parser.next_val()?)?;
        (config.offset_r, config.offset_g, config.offset_b) = (r, g, b);
      }
      Arg::Short('c') | Arg::Long("contrast") => {
        config.contrast = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('b') | Arg::Long("brightness") => {
        config.brightness = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('p') | Arg::Long("posterize") => {
        config.posterize_levels = parse_u32(parser.next_val()?)?;
      }
      Arg::Short('s') | Arg::Long("solarize") => {
        config.solarize_value = parse_f64(parser.next_val()?)?;
      }
      Arg::Short('i') | Arg::Long("invert") => config.invert = true,
      Arg::Short('x') | Arg::Long("no-red") => config.disable_r = true,
      Arg::Short('y') | Arg::Long("no-green") => config.disable_g = true,
      Arg::Short('z') | Arg::Long("no-blue") => config.disable_b = true,
      Arg::Short('h') | Arg::Long("help") => {
        write_stdout(USAGE);
      }
      _ => return None,
    }
  }
  Some(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: isize, argv: *const *mut c_char) -> c_int {
  if argc <= 0 || argv.is_null() {
    write_stderr(USAGE);
    return 1;
  }

  let mut config = Config::default();

  let args_iter = unsafe { get_args_iter(argc as usize, argv) };

  if parse_args(args_iter, &mut config).is_none() {
    bad_value()
  }

  let mut conn = match shared::connect_wayland() {
    Ok(c) => c,
    Err(e) => {
      write_wire_diagnostic("[rustemp]", &e);
      return 1;
    }
  };

  let mut state = State::init(config);

  if let Err(e) = crawl(&mut conn, &mut state) {
    write_wire_diagnostic("[rustemp]", &e);
    return 1;
  }

  // setup layer surfaces and gamma control for all monitors.
  for i in 0..state.output_len {
    // alloc ids for the new gamma control objects.
    let gamma_ctrl_id = conn.alloc_id();
    let output_id = state.outputs[i].output_id;

    // request gamma control
    let mut gamma_msg = Message::new(
      state.global.gamma_manager_id,
      zwlr_gamma_control_manager_v1::request::GET_GAMMA_CONTROL,
    );
    gamma_msg.write_u32(gamma_ctrl_id);
    gamma_msg.write_u32(output_id);
    conn.send_logged(&gamma_msg, None);

    // store the allocated ids back into our Output instance
    state.outputs[i].gamma_control_id = gamma_ctrl_id;
  }

  // main wayland event dispatch loop
  loop {
    match dispatch_once(&mut conn, &mut state) {
      Ok(()) => (),
      Err(ConnectionClosed) => break,
      Err(e) => {
        write_wire_diagnostic("[rustemp]", &e);
        break;
      }
    }
  }
  0
}
