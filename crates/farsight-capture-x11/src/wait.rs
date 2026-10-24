//! Waiting on X connection sockets with a timeout.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec, poll};

/// Waits until any of `fds` is readable or `timeout` passes, and reports
/// which ones are readable. Interrupted waits return no readiness.
pub(crate) fn readable(fds: &[&dyn AsFd], timeout: Duration) -> std::io::Result<Vec<bool>> {
    let mut polls: Vec<PollFd<'_>> = fds
        .iter()
        .map(|fd| PollFd::from_borrowed_fd(fd.as_fd(), PollFlags::IN))
        .collect();
    let timeout = Timespec::try_from(timeout).unwrap_or(Timespec {
        tv_sec: 1,
        tv_nsec: 0,
    });
    match poll(&mut polls, Some(&timeout)) {
        Ok(_) | Err(rustix::io::Errno::INTR) => {}
        Err(err) => return Err(err.into()),
    }
    Ok(polls.iter().map(|p| !p.revents().is_empty()).collect())
}

/// Wakes the thread polling the other end of `stream`. Never blocks.
pub(crate) fn wake(mut stream: &UnixStream) {
    // A full socket buffer already guarantees a pending wake-up.
    let _ = stream.write(&[1]);
}

/// Empties a wake-up socket. Returns `false` once every writer has gone.
pub(crate) fn drain(mut stream: &UnixStream) -> bool {
    let mut buf = [0u8; 64];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return false,
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => return true,
        }
    }
}
