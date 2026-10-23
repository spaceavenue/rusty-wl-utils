use core::slice;

use rustix::fd::OwnedFd;
use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use wllib::error::SysResultExt;

use crate::error::AppError;
use crate::image_load;
use crate::state::Config;

// create a memory-backed file, aka memfd, containing the scaled bgra pixel data and return its file
// descriptor to the compositor. the memfd is mmap'ed to our virtual address space, and we pass it
// as a byte slice to our image loading function. we then unmap it and send the fd back. the wayland
// side turns it into a buffer
pub fn get_image_fd(out_width: u32, out_height: u32, config: &Config) -> Result<OwnedFd, AppError> {
  let stride = out_width * 4; // 4 bytes per pixel (BGRA)
  let size = (stride * out_height) as usize;

  // create an anonymous, in-memory file.
  let fd = memfd_create(
    c"rustbg-wayland-shm",
    MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::NOEXEC_SEAL,
  )
  .syserr("memfd_create")?;

  // set the file's size, equal to our output size
  ftruncate(&fd, size as u64).syserr("ftruncate")?;

  // map the in-memory file into our address space
  let mmap_ptr = unsafe {
    mmap(
      core::ptr::null_mut(),
      size,
      ProtFlags::READ | ProtFlags::WRITE,
      MapFlags::SHARED,
      &fd,
      0,
    )
  }
  .syserr("mmap")?;

  // get a byte slice from the mmap, spawn dump-bgra/ffmpeg, fill the mmap'd region directly,
  // unmap file
  let mmap_slice = unsafe { slice::from_raw_parts_mut(mmap_ptr.cast::<u8>(), size) };
  let result = image_load::load_and_scale(out_width, out_height, mmap_slice, config);
  unsafe { munmap(mmap_ptr, size) }.syserr("munmap")?;
  result?;

  Ok(fd)
}
