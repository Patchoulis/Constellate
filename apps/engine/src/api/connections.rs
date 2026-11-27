//! Limits on connections, before a single request is read: how many there may be, and how long one may stay
//! silent.
//!
//! The checks of [`super::security`] act on requests, and a client that opens connections and then sends
//! nothing, or a byte now and then, never makes one: without these limits it could hold sockets until the engine
//! runs out of them, without knowing the token. So every connection takes a seat ([`MOST_OPEN`] in all, and
//! across a network [`MOST_PER_ADDRESS`] for each remote address), and one through which nothing has moved in
//! either direction for a while is closed. HTTP/1 also has its own deadline for a request's headers, and HTTP/2
//! pings its peer (`super::patient`); this covers what comes before either, and connections that merely idle.

use std::collections::HashMap;
use std::future::{Future, Ready, ready};
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum_server::accept::Accept;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, Sleep, sleep};

use super::config::Mode;

/// Connections open at once.
pub const MOST_OPEN: usize = 512;
/// Connections open at once from one remote address, across a network. On this computer every connection
/// comes from the same address, so there only [`MOST_OPEN`] counts.
pub const MOST_PER_ADDRESS: usize = 32;
/// How long nothing may move through a connection before it is closed.
pub const QUIET: Duration = Duration::from_secs(30);

type Counts = Arc<Mutex<HashMap<IpAddr, usize>>>;

/// The acceptor that gives each connection its seat and its deadline, or closes it at once when there is none.
#[derive(Clone)]
pub struct Seats {
    open: Arc<Semaphore>,
    by_address: Option<Counts>,
    quiet: Duration,
}

impl Seats {
    pub fn new(mode: Mode, quiet: Duration) -> Seats {
        let by_address = (mode == Mode::Network).then(Counts::default);
        Seats { open: Arc::new(Semaphore::new(MOST_OPEN)), by_address, quiet }
    }

    fn seat(&self, stream: TcpStream) -> io::Result<Watched<TcpStream>> {
        let full = || io::Error::other("too many connections");
        let permit = Arc::clone(&self.open).try_acquire_owned().map_err(|_| full())?;
        let place = match &self.by_address {
            Some(counts) => {
                let address = stream.peer_addr()?.ip().to_canonical();
                let mut open = counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let count = open.entry(address).or_insert(0);
                if *count >= MOST_PER_ADDRESS {
                    return Err(full());
                }
                *count += 1;
                Some(Place { counts: Arc::clone(counts), address })
            }
            None => None,
        };
        Ok(Watched {
            inner: stream,
            quiet: self.quiet,
            deadline: Box::pin(sleep(self.quiet)),
            _permit: permit,
            _place: place,
        })
    }
}

impl<S> Accept<TcpStream, S> for Seats {
    type Stream = Watched<TcpStream>;
    type Service = S;
    type Future = Ready<io::Result<(Watched<TcpStream>, S)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        ready(self.seat(stream).map(|watched| (watched, service)))
    }
}

/// One remote address's seat, given back when its connection closes.
struct Place {
    counts: Counts,
    address: IpAddr,
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut open = self.counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = open.get_mut(&self.address) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.address);
            }
        }
    }
}

/// A connection that fails with "timed out" once nothing has moved through it for its quiet time.
pub struct Watched<S> {
    inner: S,
    quiet: Duration,
    deadline: Pin<Box<Sleep>>,
    _permit: OwnedSemaphorePermit,
    _place: Option<Place>,
}

impl<S> Watched<S> {
    fn moved(&mut self) {
        let next = Instant::now() + self.quiet;
        self.deadline.as_mut().reset(next);
    }

    /// Waiting: fine until the deadline, then an error that closes the connection.
    fn waiting<T>(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<T>> {
        match self.deadline.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "the connection was idle"))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Watched<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.moved();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => this.waiting(cx),
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Watched<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, bytes) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    this.moved();
                }
                Poll::Ready(Ok(written))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            // a client that does not read what it is sent holds the connection as surely as one that sends nothing
            Poll::Pending => this.waiting(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// `count` connections to a listener of this computer, as the engine accepts them.
    async fn accepted(count: usize) -> (Vec<TcpStream>, Vec<TcpStream>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut clients, mut served) = (Vec::new(), Vec::new());
        for _ in 0..count {
            clients.push(TcpStream::connect(listener.local_addr().unwrap()).await.unwrap());
            served.push(listener.accept().await.unwrap().0);
        }
        (clients, served)
    }

    #[tokio::test]
    async fn across_a_network_one_address_gets_only_so_many_connections_and_a_closed_one_frees_its_seat() {
        let seats = Seats::new(Mode::Network, QUIET);
        let (_clients, served) = accepted(MOST_PER_ADDRESS + 1).await;
        let mut seated = Vec::new();
        let mut refused = 0;
        for stream in served {
            match seats.seat(stream) {
                Ok(watched) => seated.push(watched),
                Err(_) => refused += 1,
            }
        }
        assert_eq!((seated.len(), refused), (MOST_PER_ADDRESS, 1));
        seated.pop();
        let (_more, mut again) = accepted(1).await;
        assert!(seats.seat(again.remove(0)).is_ok());
    }

    #[tokio::test]
    async fn on_this_computer_only_the_total_counts() {
        let seats = Seats::new(Mode::Local, QUIET);
        let (_clients, served) = accepted(MOST_PER_ADDRESS + 1).await;
        assert!(served.into_iter().all(|stream| seats.seat(stream).is_ok()));
    }

    #[tokio::test]
    async fn a_connection_through_which_nothing_moves_is_closed() {
        let seats = Seats::new(Mode::Local, Duration::from_millis(100));
        let (_clients, mut served) = accepted(1).await;
        let mut watched = seats.seat(served.remove(0)).unwrap();
        let mut buffer = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(10), watched.read(&mut buffer)).await.unwrap();
        assert_eq!(read.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
