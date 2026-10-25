// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sockets a session is blocked on, for ending it from another thread.
//!
//! A protocol thread spends most of its time in a socket call (a read, or,
//! before there is anything to read, the dial), and only the socket itself can
//! interrupt one: shut it down and the call returns. Every client here did
//! that for the connected socket and none of them for the dial, because
//! `TcpStream::connect_timeout` creates its socket internally and gives no
//! handle to it. A disconnect while a black-holed address was being dialled
//! therefore waited out the whole connect timeout, and a `destroy` after it
//! blocked the window's thread, joined on the session's, for twenty seconds.
//!
//! So the dial is done here instead, on a socket that is held from the moment
//! its connect has begun, and [`Hangup::hang_up`] shuts down whatever is held
//! at the time. Shutting down a socket whose connect is still in flight aborts
//! the connect (Linux's `inet_shutdown` disconnects a `SYN_SENT` socket and
//! wakes the threads waiting on it), so one mechanism covers both the dial and
//! the connected session.
//!
//! Name resolution is not interruptible: the C library does it and has no
//! handle to interrupt, so an address the resolver takes twenty seconds to
//! fail still takes twenty seconds. A literal address resolves at once.

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A flag and the sockets it breaks. One per session.
#[derive(Default)]
pub struct Hangup {
    hung_up: AtomicBool,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    next: u64,
    sockets: Vec<(u64, TcpStream)>,
}

/// A socket [`Hangup::hold`] is holding, until this is dropped. Each is a
/// second descriptor for the same socket, so letting go of it closes nothing
/// the caller still has.
#[must_use = "the socket is held only while this is"]
pub struct Holding<'a> {
    hangup: &'a Hangup,
    id: u64,
}

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        let mut held = self.hangup.held.lock().unwrap();
        held.sockets.retain(|(id, _)| *id != self.id);
    }
}

impl Hangup {
    pub fn new() -> Hangup {
        Hangup::default()
    }

    /// Ends whatever is held, and refuses anything held from now on. Safe to
    /// call from any thread, any number of times.
    pub fn hang_up(&self) {
        // The flag before the lock, and `hold` tests it under the lock: a
        // socket is either held in time to be shut down here or refused there,
        // and never slips between the two.
        self.hung_up.store(true, Ordering::Release);
        let held = self.held.lock().unwrap();
        for (_, socket) in &held.sockets {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    pub fn is_hung_up(&self) -> bool {
        self.hung_up.load(Ordering::Acquire)
    }

    /// Holds `socket` until the returned guard is dropped, so that a hang-up
    /// shuts it down. Refused, with the socket shut down, once the session has
    /// been hung up.
    pub fn hold(&self, socket: &TcpStream) -> io::Result<Holding<'_>> {
        let mut held = self.held.lock().unwrap();
        if self.is_hung_up() {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(closed());
        }
        let id = held.next;
        held.next += 1;
        held.sockets.push((id, socket.try_clone()?));
        Ok(Holding { hangup: self, id })
    }

    /// `TcpStream::connect_timeout`, but a hang-up ends it. The socket comes
    /// back blocking, as std's does.
    pub fn dial(&self, addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
        self.dial_from(socket, addr, timeout)
    }

    /// [`Hangup::dial`] on a socket the caller has already set up, such as one
    /// bound to a particular port for a hole punch.
    pub fn dial_from(&self, socket: Socket, addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        socket.set_nonblocking(true)?;
        match socket.connect(&SockAddr::from(addr)) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) || e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        let stream = TcpStream::from(socket);
        // Held only now that the connect has begun: shutting down a socket
        // before `connect` is called does not stop `connect` from using it.
        let holding = self.hold(&stream)?;
        let waited = wait_connected(&stream, timeout);
        drop(holding);
        match waited {
            Err(_) if self.is_hung_up() => Err(closed()),
            Err(e) => Err(e),
            Ok(()) => {
                stream.set_nonblocking(false)?;
                Ok(stream)
            }
        }
    }
}

/// What a dial or a hold says once the session has been hung up.
fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "the session was closed")
}

/// The other half of a non-blocking connect: writable means done, and the
/// socket's pending error says how.
fn wait_connected(stream: &TcpStream, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let ms = left.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        // SAFETY: one pollfd, which lives across the call.
        match unsafe { libc::poll(&mut fd, 1, ms) } {
            0 => return Err(io::ErrorKind::TimedOut.into()),
            n if n < 0 => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
            _ => {
                return match stream.take_error()? {
                    Some(e) => Err(e),
                    // POLLHUP with no error is a socket shut down before it
                    // ever connected.
                    None if fd.revents & libc::POLLHUP != 0 => Err(io::ErrorKind::NotConnected.into()),
                    None => Ok(()),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;

    #[test]
    fn a_dial_to_a_listener_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        let stream = hangup
            .dial(listener.local_addr().unwrap(), Duration::from_secs(5))
            .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), listener.local_addr().unwrap());
        // Blocking again, as std's connect leaves it.
        stream.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let mut byte = [0u8];
        let e = std::io::Read::read(&mut &stream, &mut byte).unwrap_err();
        assert!(matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut));
    }

    #[test]
    fn a_refused_dial_says_so() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let e = Hangup::new().dial(port, Duration::from_secs(5)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
    }

    /// The reason this exists: a dial that would sit out its timeout ends when
    /// the session is hung up. The address is one that drops what is sent to
    /// it; where the network says at once that it is unreachable instead, the
    /// dial ends at once too, and the bound below still holds.
    #[test]
    fn a_hang_up_ends_a_dial_in_flight() {
        let hangup = Arc::new(Hangup::new());
        let started = Instant::now();
        let dialling = {
            let hangup = Arc::clone(&hangup);
            std::thread::spawn(move || hangup.dial("10.255.255.1:9".parse().unwrap(), Duration::from_secs(20)))
        };
        std::thread::sleep(Duration::from_millis(200));
        hangup.hang_up();
        let e = dialling.join().unwrap().unwrap_err();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "took {took:?}");
        // Still dialling when it was hung up, so it was the hang-up that ended it.
        if took >= Duration::from_millis(200) {
            assert_eq!(e.kind(), io::ErrorKind::Interrupted, "{e}");
        }
    }

    #[test]
    fn nothing_is_held_after_a_hang_up() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        hangup.hang_up();
        let e = hangup
            .dial(listener.local_addr().unwrap(), Duration::from_secs(5))
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn a_hang_up_breaks_a_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        let stream = hangup
            .dial(listener.local_addr().unwrap(), Duration::from_secs(5))
            .unwrap();
        let _accepted = listener.accept().unwrap();
        let holding = hangup.hold(&stream).unwrap();
        std::thread::scope(|scope| {
            let reading = scope.spawn(|| {
                let mut byte = [0u8];
                std::io::Read::read(&mut &stream, &mut byte)
            });
            std::thread::sleep(Duration::from_millis(100));
            hangup.hang_up();
            // A shut-down socket reads as the end of the stream.
            assert_eq!(reading.join().unwrap().unwrap(), 0);
        });
        drop(holding);
    }

    #[test]
    fn a_dropped_holding_lets_go() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        drop(hangup.hold(&stream).unwrap());
        assert!(hangup.held.lock().unwrap().sockets.is_empty());
    }
}
