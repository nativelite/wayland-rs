//! The socket calls the standard library does not have: `sendmsg` and
//! `recvmsg` with file descriptors beside the bytes, and `poll`. Declared
//! here against the C library the standard library already links; the
//! layouts and constants are Linux's on the architectures `conn` is built
//! for.

#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "x86",
    target_arch = "aarch64",
    target_arch = "arm",
    target_arch = "riscv64"
)))]
compile_error!(
    "nativelite-wayland: the socket constants in conn/sys.rs are not verified for this architecture"
);

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

#[repr(C)]
struct IoVec {
    base: *mut c_void,
    len: usize,
}

/// The kernel's `struct msghdr`; `repr(C)` pads after `namelen` and
/// `flags` as C does.
#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    namelen: u32,
    iov: *mut IoVec,
    iovlen: usize,
    control: *mut c_void,
    controllen: usize,
    flags: i32,
}

#[repr(C)]
struct CmsgHdr {
    len: usize,
    level: i32,
    kind: i32,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

extern "C" {
    fn sendmsg(fd: i32, msg: *const MsgHdr, flags: i32) -> isize;
    fn recvmsg(fd: i32, msg: *mut MsgHdr, flags: i32) -> isize;
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout_ms: i32) -> i32;
    // Variadic, exactly as C declares it.
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const MSG_CTRUNC: i32 = 0x8;
const MSG_DONTWAIT: i32 = 0x40;
const MSG_NOSIGNAL: i32 = 0x4000;
const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;
const POLLIN: i16 = 0x1;
const POLLOUT: i16 = 0x4;
const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;

/// The most descriptors sent with one `sendmsg`, as libwayland sends.
pub(super) const MAX_FDS_OUT: usize = 28;
/// The most descriptors one `recvmsg` can bring: the kernel's limit per
/// message (`SCM_MAX_FD`).
const MAX_FDS_IN: usize = 253;

const HDR: usize = size_of::<CmsgHdr>();

/// A control message's length rounded up to the kernel's alignment.
const fn align(len: usize) -> usize {
    (len + size_of::<usize>() - 1) & !(size_of::<usize>() - 1)
}

/// Room for one control message carrying `MAX_FDS_IN` descriptors, in
/// words so it is aligned for a `CmsgHdr`.
const CONTROL_WORDS: usize = (align(HDR + MAX_FDS_IN * 4) + 7) / 8;

/// Sends some of `bytes` without blocking, with `fds` beside the first
/// byte. Returns how many bytes went (the descriptors went if any did), or
/// `WouldBlock`.
pub(super) fn send(socket: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    debug_assert!(fds.len() <= MAX_FDS_OUT && !bytes.is_empty());
    let mut control = [0u64; CONTROL_WORDS];
    let mut iov = IoVec {
        base: bytes.as_ptr() as *mut c_void,
        len: bytes.len(),
    };
    let mut msg = MsgHdr {
        name: std::ptr::null_mut(),
        namelen: 0,
        iov: &mut iov,
        iovlen: 1,
        control: std::ptr::null_mut(),
        controllen: 0,
        flags: 0,
    };
    if !fds.is_empty() {
        let len = HDR + fds.len() * 4;
        let base = control.as_mut_ptr() as *mut u8;
        // SAFETY: `control` is aligned for a CmsgHdr and holds MAX_FDS_IN
        // descriptors after one, more than the MAX_FDS_OUT written here.
        unsafe {
            (base as *mut CmsgHdr).write(CmsgHdr {
                len,
                level: SOL_SOCKET,
                kind: SCM_RIGHTS,
            });
            let data = base.add(HDR) as *mut i32;
            for (i, fd) in fds.iter().enumerate() {
                data.add(i).write(*fd);
            }
        }
        msg.control = base as *mut c_void;
        msg.controllen = align(len);
    }
    loop {
        // SAFETY: `msg` points at one iovec over `bytes` and, when there
        // are descriptors, at the control message built above; all of it
        // outlives the call.
        let n = unsafe { sendmsg(socket, &msg, MSG_DONTWAIT | MSG_NOSIGNAL) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Reads what is there into `buf` without blocking, appending descriptors
/// that came with it to `fds` (close-on-exec set). Returns the number of
/// bytes, 0 at end of stream, or `WouldBlock`.
pub(super) fn recv(
    socket: RawFd,
    buf: &mut [u8],
    fds: &mut VecDeque<OwnedFd>,
) -> io::Result<usize> {
    let mut control = [0u64; CONTROL_WORDS];
    let mut iov = IoVec {
        base: buf.as_mut_ptr() as *mut c_void,
        len: buf.len(),
    };
    let mut msg = MsgHdr {
        name: std::ptr::null_mut(),
        namelen: 0,
        iov: &mut iov,
        iovlen: 1,
        control: control.as_mut_ptr() as *mut c_void,
        controllen: size_of::<[u64; CONTROL_WORDS]>(),
        flags: 0,
    };
    let n = loop {
        // SAFETY: `msg` points at one iovec over `buf` and at `control`,
        // with their true lengths; both outlive the call.
        let n = unsafe { recvmsg(socket, &mut msg, MSG_DONTWAIT | MSG_CMSG_CLOEXEC) };
        if n >= 0 {
            break n as usize;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    };
    // Take ownership of every descriptor the kernel installed, whatever
    // else is wrong, so none leaks.
    let base = control.as_ptr() as *const u8;
    let end = msg.controllen.min(size_of::<[u64; CONTROL_WORDS]>());
    let mut at = 0;
    while at + HDR <= end {
        // SAFETY: `at + HDR <= end` keeps the header inside `control`, and
        // `at` is a multiple of the word size, so it is aligned.
        let hdr = unsafe { (base.add(at) as *const CmsgHdr).read() };
        if hdr.len < HDR || hdr.len > end - at {
            break;
        }
        if hdr.level == SOL_SOCKET && hdr.kind == SCM_RIGHTS {
            for i in 0..(hdr.len - HDR) / 4 {
                // SAFETY: inside the control message, by the length check
                // above; the kernel wrote a descriptor that is ours now.
                let fd = unsafe { (base.add(at + HDR + i * 4) as *const i32).read_unaligned() };
                fds.push_back(unsafe { OwnedFd::from_raw_fd(fd) });
            }
        }
        at += align(hdr.len);
    }
    if msg.flags & MSG_CTRUNC != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "more file descriptors arrived in one message than can be received",
        ));
    }
    Ok(n)
}

/// What a socket is ready for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ready {
    /// There is something to read, or the peer hung up, or an error is
    /// waiting: a read will say which.
    pub read: bool,
    /// A write would not block.
    pub write: bool,
}

/// Waits until `socket` can be read (or written too, with `write`), for at
/// most `timeout`, forever with `None`. Nothing ready means it timed out.
pub(super) fn wait(socket: RawFd, write: bool, timeout: Option<Duration>) -> io::Result<Ready> {
    let deadline = timeout.and_then(|d| std::time::Instant::now().checked_add(d));
    let mut fd = PollFd {
        fd: socket,
        events: POLLIN | if write { POLLOUT } else { 0 },
        revents: 0,
    };
    loop {
        // Worked out again each time round: a signal that interrupts the
        // wait does not restart the timeout.
        let ms = match (timeout, deadline) {
            (None, _) => -1,
            (Some(_), None) => i32::MAX,
            (Some(_), Some(at)) => {
                let left = at.saturating_duration_since(std::time::Instant::now());
                // Round up, so a short timeout does not become a busy loop.
                i32::try_from((left.as_nanos() + 999_999) / 1_000_000).unwrap_or(i32::MAX)
            }
        };
        // SAFETY: one pollfd, and the count says one.
        let n = unsafe { poll(&mut fd, 1, ms) };
        if n >= 0 {
            return Ok(Ready {
                read: fd.revents & !POLLOUT != 0,
                write: fd.revents & POLLOUT != 0,
            });
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Marks `fd` close-on-exec.
pub(super) fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl with F_GETFD/F_SETFD takes no pointers.
    unsafe {
        let flags = fcntl(fd, F_GETFD);
        if flags < 0 || fcntl(fd, F_SETFD, flags | FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
