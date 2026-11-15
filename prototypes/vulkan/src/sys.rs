//! The few Linux calls the standard library does not wrap: a shared-memory file and its mapping,
//! as a `wl_shm` client makes, anonymous memory to compare with, and waiting on a sync file.
//! Declared here rather than taken from the `libc` crate: `ash` is the prototype's only
//! dependency.

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;

const MFD_CLOEXEC: c_uint = 0x1;
const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const MAP_SHARED: c_int = 0x1;
const MAP_PRIVATE: c_int = 0x2;
const MAP_ANONYMOUS: c_int = 0x20;
const POLLIN: i16 = 0x1;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

// SAFETY: these are glibc's declarations of the calls on x86_64 Linux (`off_t` and `nfds_t` are
// 64 bits there), the only target the flake builds for.
unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
    fn poll(fds: *mut PollFd, count: u64, timeout_ms: c_int) -> c_int;
}

/// Memory mapped for reading and writing: a `wl_shm` client's file, or anonymous memory.
pub struct Mapping {
    /// The file behind the mapping, held open while it lives; none for anonymous memory.
    _file: Option<File>,
    pointer: NonNull<u8>,
    length: usize,
}

impl Mapping {
    /// A new memory file of `length` bytes, zeroed, mapped shared, as a `wl_shm` client shares
    /// its pixels.
    pub fn memfd(name: &CStr, length: usize) -> io::Result<Self> {
        // SAFETY: `name` is a NUL-terminated string that lives for the call.
        let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: memfd_create returned a new descriptor that nothing else owns.
        let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        file.set_len(length as u64)?;
        let pointer = map(length, MAP_SHARED, file.as_raw_fd())?;
        Ok(Self {
            _file: Some(file),
            pointer,
            length,
        })
    }

    /// `length` bytes of anonymous memory, zeroed, private to the process.
    pub fn anonymous(length: usize) -> io::Result<Self> {
        Ok(Self {
            _file: None,
            pointer: map(length, MAP_PRIVATE | MAP_ANONYMOUS, -1)?,
            length,
        })
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.pointer.as_ptr()
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the mapping covers `length` bytes for as long as `self` lives, and no other
        // process holds its file: only this process and the GPU it hands the pages to touch them,
        // and the GPU only reads.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.length) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as in `as_slice`; `&mut self` makes this the only reference to the bytes.
        unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.length) }
    }
}

/// Maps `length` bytes of `fd` (-1 for anonymous memory) for reading and writing.
fn map(length: usize, flags: c_int, fd: c_int) -> io::Result<NonNull<u8>> {
    // SAFETY: `fd` is -1 with MAP_ANONYMOUS, or an open file of at least `length` bytes; the
    // kernel picks the address, so no existing mapping is replaced.
    let address = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            flags,
            fd,
            0,
        )
    };
    // MAP_FAILED is (void *) -1.
    if address as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    NonNull::new(address.cast::<u8>())
        .ok_or_else(|| io::Error::other("mmap returned a null address"))
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the mapping was made by `map` with this address and length, and no slice of it
        // outlives `self`. Its failure would leave the mapping until the process ends.
        unsafe {
            munmap(self.pointer.as_ptr().cast(), self.length);
        }
    }
}

/// Whether a sync file signals within `timeout_ms`: it polls readable once signalled.
pub fn sync_file_signals(fd: &OwnedFd, timeout_ms: i32) -> io::Result<bool> {
    let mut entry = PollFd {
        fd: fd.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    };
    // SAFETY: `entry` is one valid pollfd that lives for the call, and the descriptor is open.
    let ready = unsafe { poll(&mut entry, 1, timeout_ms) };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ready == 1 && entry.revents & POLLIN != 0)
}
