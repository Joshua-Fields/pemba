//! Counts the round trips a Postgres client makes (OQ-056), for the Postgres benches.
//!
//! A TCP proxy between the store and the server that parses the client's side of the
//! wire protocol (v3, plaintext) and counts each request the client then waits on: a
//! simple `Query`, or an extended-protocol group (`Parse`, `Bind`, `Describe`, `Execute`)
//! ended by `Sync`. A group holding only `Close` is not counted: the `postgres` crate sends
//! it when a statement is dropped and never waits for the reply. Counting the protocol
//! rather than instrumenting the store means the count does not depend on the code being
//! measured.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

/// `SSLRequest` and `GSSENCRequest` codes: untyped messages that precede the startup one.
const SSL_REQUEST: u32 = 80_877_103;
const GSSENC_REQUEST: u32 = 80_877_104;

/// A running proxy; it serves until the process exits.
#[derive(Debug)]
pub struct RoundTripProxy {
    addr: SocketAddr,
    count: Arc<AtomicU64>,
}

impl RoundTripProxy {
    /// Listens on a free local port and forwards each connection to `upstream`.
    ///
    /// # Errors
    ///
    /// If the listener cannot bind or `upstream` does not resolve.
    pub fn start(upstream: impl ToSocketAddrs) -> io::Result<Self> {
        let upstream = upstream
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upstream did not resolve"))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let count = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&count);
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let c = Arc::clone(&c);
                thread::spawn(move || {
                    // A broken connection only ends that connection's count.
                    let _ = serve(client, upstream, &c);
                });
            }
        });
        Ok(Self { addr, count })
    }

    /// The address to connect to instead of the server.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Round trips counted so far, over every connection.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }
}

fn serve(mut client: TcpStream, upstream: SocketAddr, count: &AtomicU64) -> io::Result<()> {
    let mut server = TcpStream::connect(upstream)?;
    client.set_nodelay(true)?;
    server.set_nodelay(true)?;
    let mut back_from = server.try_clone()?;
    let mut back_to = client.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut back_from, &mut back_to);
        let _ = back_to.shutdown(Shutdown::Both);
    });
    let result = forward(&mut client, &mut server, count);
    let _ = server.shutdown(Shutdown::Both);
    result
}

fn forward(client: &mut TcpStream, server: &mut TcpStream, count: &AtomicU64) -> io::Result<()> {
    // Untyped messages first: optional encryption requests, then the startup message.
    loop {
        let mut len = [0u8; 4];
        client.read_exact(&mut len)?;
        let body = read_body(client, u32::from_be_bytes(len))?;
        server.write_all(&len)?;
        server.write_all(&body)?;
        let code = body
            .get(..4)
            .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        if code != SSL_REQUEST && code != GSSENC_REQUEST {
            break;
        }
    }
    let mut waits = false;
    loop {
        let mut head = [0u8; 5];
        client.read_exact(&mut head)?;
        let body = read_body(
            client,
            u32::from_be_bytes([head[1], head[2], head[3], head[4]]),
        )?;
        server.write_all(&head)?;
        server.write_all(&body)?;
        match head[0] {
            b'Q' => {
                count.fetch_add(1, Ordering::SeqCst);
            }
            b'P' | b'B' | b'D' | b'E' => waits = true,
            b'S' => {
                if waits {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                waits = false;
            }
            b'X' => return Ok(()),
            _ => {}
        }
    }
}

/// The rest of a message whose length field (which counts itself) was `len`.
fn read_body(r: &mut impl Read, len: u32) -> io::Result<Vec<u8>> {
    let n = usize::try_from(len.saturating_sub(4))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message too long"))?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    Ok(body)
}
