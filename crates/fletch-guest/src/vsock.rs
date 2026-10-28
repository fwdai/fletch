//! AF_VSOCK sockets through libc. Linux only: elsewhere every call fails with
//! `Unsupported`, so the crate still compiles and its unit tests run on macOS.
//!
//! Streams are handed out as `UnixStream`, which only needs a connected stream
//! fd: read, write, `try_clone`, `shutdown` and `From<UnixStream> for Stdio`
//! all work on a vsock socket.

use std::io;
use std::os::unix::net::UnixStream;

#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::mem::size_of;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    pub struct Listener(OwnedFd);

    fn check(ret: libc::c_int) -> io::Result<libc::c_int> {
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret)
        }
    }

    fn socket() -> io::Result<OwnedFd> {
        let fd = check(unsafe {
            libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0)
        })?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn addr(cid: u32, port: u32) -> libc::sockaddr_vm {
        let mut a: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
        a.svm_family = libc::AF_VSOCK as libc::sa_family_t;
        a.svm_cid = cid;
        a.svm_port = port;
        a
    }

    pub fn listen(port: u32) -> io::Result<Listener> {
        let fd = socket()?;
        let a = addr(libc::VMADDR_CID_ANY, port);
        check(unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &a as *const libc::sockaddr_vm as *const libc::sockaddr,
                size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        })?;
        check(unsafe { libc::listen(fd.as_raw_fd(), 16) })?;
        Ok(Listener(fd))
    }

    impl Listener {
        pub fn accept(&self) -> io::Result<UnixStream> {
            let fd = check(unsafe {
                libc::accept4(
                    self.0.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC,
                )
            })?;
            Ok(UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) }))
        }
    }

    /// Connects to `port` on the host (CID 2).
    pub fn connect(port: u32) -> io::Result<UnixStream> {
        let fd = socket()?;
        let a = addr(libc::VMADDR_CID_HOST, port);
        check(unsafe {
            libc::connect(
                fd.as_raw_fd(),
                &a as *const libc::sockaddr_vm as *const libc::sockaddr,
                size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        })?;
        Ok(UnixStream::from(fd))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::io;
    use std::os::unix::net::UnixStream;

    pub struct Listener(());

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "AF_VSOCK is Linux-only")
    }

    pub fn listen(_port: u32) -> io::Result<Listener> {
        Err(unsupported())
    }

    impl Listener {
        pub fn accept(&self) -> io::Result<UnixStream> {
            Err(unsupported())
        }
    }

    pub fn connect(_port: u32) -> io::Result<UnixStream> {
        Err(unsupported())
    }
}

pub use imp::Listener;

pub fn listen(port: u32) -> io::Result<Listener> {
    imp::listen(port)
}

pub fn connect(port: u32) -> io::Result<UnixStream> {
    imp::connect(port)
}
