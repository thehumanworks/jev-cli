//! One-shot HTTP/1.1 server for the binary tests. No extra crates: `std::net` only.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// A server bound to `127.0.0.1:0` that answers one request.
pub(crate) struct Server {
    /// The ephemeral port.
    pub(crate) port: u16,
    request: Receiver<String>,
}

impl Server {
    /// The raw HTTP request, headers and body.
    pub(crate) fn request(&self) -> String {
        self.request.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
    }
}

/// Accept one connection and respond with `status` and `body`.
pub(crate) fn spawn(status: u16, body: &str) -> Server {
    spawn_all(&[(status, body)])
}

/// Accept one connection per response, in order. Each response closes the connection.
pub(crate) fn spawn_all(responses: &[(u16, &str)]) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let responses: Vec<(u16, String)> = responses.iter().map(|(status, body)| (*status, (*body).to_owned())).collect();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body) in responses {
            let Ok(mut stream) = accept_for(&listener, Duration::from_secs(5)) else {
                return;
            };
            let raw = read_request(&mut stream).unwrap_or_default();
            let _ = tx.send(raw);
            let header = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reason(status),
                body.len(),
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
    });
    Server { port, request: rx }
}

fn accept_for(listener: &TcpListener, timeout: Duration) -> std::io::Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if start.elapsed() > timeout {
                    return Err(std::io::Error::new(ErrorKind::TimedOut, "no client"));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 4096];
    loop {
        if request_complete(&buf) {
            break;
        }
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut => break,
            Err(err) => return Err(err),
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn request_complete(buf: &[u8]) -> bool {
    let Some(end) = find_subsequence(buf, b"\r\n\r\n") else {
        return false;
    };
    let header_end = end.saturating_add(4);
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    content_length(&headers).is_some_and(|len| buf.len() >= header_end.saturating_add(len))
}

fn content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
