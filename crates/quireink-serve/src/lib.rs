//! Serving HTTP for plugins, for shells with the standard library (the
//! emulator, Android): a small HTTP/1.1 server per plugin server, each
//! connection on a thread of its own, one request per connection. Also
//! plugins' sockets ([`Sockets`]).
//!
//! The shell starts and stops servers as the kernel asks
//! (`ServerCall`), passes requests on (`ServerEvent`) and the answers
//! back. Bodies go in parts both ways, so they never need to fit in the
//! kernel's memory: request bodies as `ServerEvent::Body`, files sent as
//! answers through [`Servers::pump`]. The channels in between are
//! bounded, so a fast network waits for a slow storage, not the other
//! way round.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{
    Receiver, Sender, SyncSender, TryRecvError, TrySendError, channel, sync_channel,
};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use quireink_core::{ServedFile, ServerCall, ServerEvent};

mod sockets;

pub use sockets::Sockets;

/// Size of the parts bodies go in.
const PART: usize = 64 * 1024;
/// Events waiting at most.
const QUEUE: usize = 8;
/// Parts of a file read ahead at most, per connection. The shell is
/// woken each time half of them were sent, so it reads many at once.
const READ_AHEAD: usize = 32;

/// An answer for a waiting connection.
struct Answer {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// Instead of `body`: its size, and its parts as they are read.
    file: Option<(u64, Receiver<Vec<u8>>)>,
}

/// A file being sent.
struct Stream {
    path: String,
    size: u64,
    at: u64,
    parts: SyncSender<Vec<u8>>,
    /// A part read but not taken yet.
    held: Option<Vec<u8>>,
}

type Waiting = Arc<Mutex<HashMap<u32, Sender<Answer>>>>;
type Wake = Arc<dyn Fn() + Send + Sync>;

/// The servers of all plugins.
pub struct Servers {
    events: SyncSender<ServerEvent>,
    receiver: Receiver<ServerEvent>,
    running: HashMap<u32, Arc<AtomicBool>>,
    waiting: Waiting,
    streams: Vec<Stream>,
    next_request: Arc<AtomicU32>,
    /// Called when there is something to do, e.g. to wake an event loop.
    wake: Wake,
}

impl std::fmt::Debug for Servers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Servers")
            .field("running", &self.running.len())
            .field("streams", &self.streams.len())
            .finish_non_exhaustive()
    }
}

impl Servers {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (events, receiver) = sync_channel(QUEUE);
        Self {
            events,
            receiver,
            running: HashMap::new(),
            waiting: Arc::default(),
            streams: Vec::new(),
            next_request: Arc::new(AtomicU32::new(0)),
            wake: Arc::new(wake),
        }
    }

    /// Does what the kernel asks.
    pub fn apply(&mut self, call: ServerCall) {
        match call {
            ServerCall::Listen { server, port } => self.listen(server, port),
            ServerCall::Stop { server } => {
                if let Some(stop) = self.running.remove(&server) {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            ServerCall::Respond {
                request,
                status,
                content_type,
                headers,
                body,
                file,
            } => {
                let Some(answer) = self
                    .waiting
                    .lock()
                    .ok()
                    .and_then(|mut w| w.remove(&request))
                else {
                    return;
                };
                let file = file.map(|ServedFile { path, size }| {
                    let (parts, receiver) = sync_channel(READ_AHEAD);
                    self.streams.push(Stream {
                        path,
                        size,
                        at: 0,
                        parts,
                        held: None,
                    });
                    (size, receiver)
                });
                let _ = answer.send(Answer {
                    status,
                    content_type,
                    headers,
                    body,
                    file,
                });
            }
        }
    }

    /// An event to pass to the kernel, if any.
    pub fn next(&self) -> Option<ServerEvent> {
        self.receiver.try_recv().ok()
    }

    /// Reads the next parts of the files being sent, as far as the
    /// connections take them; `read` reads part of a file from `at`
    /// (`Kernel::read_served`). Call after the events were passed on.
    pub fn pump(&mut self, mut read: impl FnMut(&str, u64, &mut [u8]) -> Option<usize>) {
        let mut buffer = Vec::new();
        self.streams.retain_mut(|stream| {
            loop {
                let part = match stream.held.take() {
                    Some(part) => part,
                    None if stream.at >= stream.size => return false,
                    None => {
                        buffer.resize(PART, 0);
                        let want = (stream.size - stream.at).min(PART as u64) as usize;
                        match read(&stream.path, stream.at, &mut buffer[..want]) {
                            Some(n) if n > 0 => buffer[..n].to_vec(),
                            // Gone or shorter than it was: the connection
                            // ends early.
                            _ => return false,
                        }
                    }
                };
                let len = part.len() as u64;
                match stream.parts.try_send(part) {
                    Ok(()) => stream.at += len,
                    Err(TrySendError::Full(part)) => {
                        stream.held = Some(part);
                        return true;
                    }
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
        });
    }

    /// Whether any server runs or a file is being sent, e.g. to keep
    /// polling.
    pub fn any(&self) -> bool {
        !self.running.is_empty() || !self.streams.is_empty()
    }

    fn listen(&mut self, server: u32, port: u16) {
        let events = self.events.clone();
        let wake = self.wake.clone();
        // The shell is woken once the event is there to take, and before
        // waiting for room.
        let send = move |event: ServerEvent| {
            match events.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    wake();
                    if events.send(event).is_err() {
                        return;
                    }
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
            wake();
        };
        // The port asked for, else the next free ones.
        let listener = (port..port.saturating_add(20))
            .find_map(|p| TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).ok());
        let Some(listener) = listener else {
            send(ServerEvent::Failed {
                server,
                reason: format!("port {port} is taken"),
            });
            return;
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let _ = listener.set_nonblocking(true);
        let stop = Arc::new(AtomicBool::new(false));
        self.running.insert(server, stop.clone());
        send(ServerEvent::Started {
            server,
            address: format!("{}:{port}", local_address()),
        });
        let (waiting, next, wake) = (
            self.waiting.clone(),
            self.next_request.clone(),
            self.wake.clone(),
        );
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let connection = Connection {
                            server,
                            send: send.clone(),
                            waiting: waiting.clone(),
                            next: next.clone(),
                            wake: wake.clone(),
                        };
                        thread::spawn(move || connection.handle(stream));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            }
        });
    }
}

/// This device's address in the local network (where the default route
/// goes; no packet is sent), else the loopback address.
pub fn local_address() -> IpAddr {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect(("8.8.8.8", 80))?;
            socket.local_addr()
        })
        .map(|address| address.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// One connection: one request and its answer.
struct Connection<F> {
    server: u32,
    send: F,
    waiting: Waiting,
    next: Arc<AtomicU32>,
    wake: Wake,
}

impl<F: Fn(ServerEvent)> Connection<F> {
    fn handle(self, stream: TcpStream) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return;
        };
        let (method, target) = (method.to_string(), target.to_string());
        let mut length = 0u64;
        let mut chunked = false;
        let mut header_bytes = 0;
        loop {
            let mut header = String::new();
            match reader.read_line(&mut header) {
                Ok(0) | Err(_) => return,
                Ok(n) => header_bytes += n,
            }
            if header_bytes > 64 * 1024 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                let (name, value) = (name.trim(), value.trim());
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.parse().unwrap_or(0);
                } else if name.eq_ignore_ascii_case("transfer-encoding") {
                    chunked = !value.eq_ignore_ascii_case("identity");
                }
            }
        }
        if chunked {
            let _ = write_head(&mut writer, 411, "text/plain", &[], 0);
            return;
        }
        let request = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (answer, receiver) = channel();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(request, answer);
        }
        (self.send)(ServerEvent::Request {
            server: self.server,
            request,
            method,
            target,
            body: length > 0,
        });
        let mut left = length;
        while left > 0 {
            let mut part = vec![0u8; left.min(PART as u64) as usize];
            if reader.read_exact(&mut part).is_err() {
                return self.close(request);
            }
            left -= part.len() as u64;
            (self.send)(ServerEvent::Body {
                request,
                data: part,
                last: left == 0,
            });
        }
        let Ok(answer) = receiver.recv_timeout(Duration::from_secs(120)) else {
            let _ = write_head(&mut writer, 503, "text/plain", &[], 0);
            return self.close(request);
        };
        let size = answer
            .file
            .as_ref()
            .map_or(answer.body.len() as u64, |f| f.0);
        let written = write_head(
            &mut writer,
            answer.status,
            &answer.content_type,
            &answer.headers,
            size,
        )
        .and_then(|()| writer.write_all(&answer.body));
        if written.is_err() {
            return;
        }
        if let Some((_, parts)) = answer.file {
            let mut sent = 0;
            loop {
                let part = match parts.try_recv() {
                    Ok(part) => part,
                    Err(TryRecvError::Disconnected) => break,
                    // Nothing read ahead: the shell reads on.
                    Err(TryRecvError::Empty) => {
                        (self.wake)();
                        match parts.recv_timeout(Duration::from_secs(60)) {
                            Ok(part) => part,
                            Err(_) => break,
                        }
                    }
                };
                sent += 1;
                if sent % (READ_AHEAD / 2) == 0 {
                    (self.wake)();
                }
                if writer.write_all(&part).is_err() {
                    return;
                }
            }
        }
        let _ = writer.flush();
    }

    /// The connection was lost: the kernel forgets the request.
    fn close(&self, request: u32) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(&request);
        }
        (self.send)(ServerEvent::Closed { request });
    }
}

fn write_head(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    headers: &[(String, String)],
    length: u64,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        411 => "Length Required",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nConnection: close\r\nCache-Control: no-store\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(servers: &Servers) -> ServerEvent {
        servers
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
    }

    #[test]
    fn serves_a_request_in_parts() {
        let mut servers = Servers::new(|| {});
        servers.apply(ServerCall::Listen {
            server: 1,
            port: 18_765,
        });
        let ServerEvent::Started { address, .. } = next(&servers) else {
            panic!("not started");
        };
        let port: u16 = address.rsplit(':').next().unwrap().parse().unwrap();
        // A body of several parts goes up; a file of several parts comes
        // down.
        let up: Vec<u8> = (0..PART * 2 + 10).map(|i| i as u8).collect();
        let down: Vec<u8> = (0..PART * 3 + 7).map(|i| (i * 7) as u8).collect();
        let sent = up.clone();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let head = format!(
                "PUT /up?x=1 HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n",
                sent.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&sent).unwrap();
            let mut answer = Vec::new();
            stream.read_to_end(&mut answer).unwrap();
            answer
        });
        let ServerEvent::Request {
            request,
            method,
            target,
            body,
            ..
        } = next(&servers)
        else {
            panic!("no request");
        };
        assert_eq!(
            (method.as_str(), target.as_str(), body),
            ("PUT", "/up?x=1", true)
        );
        let mut received = Vec::new();
        loop {
            let ServerEvent::Body { data, last, .. } = next(&servers) else {
                panic!("no body");
            };
            assert!(data.len() <= PART);
            received.extend(data);
            if last {
                break;
            }
        }
        assert_eq!(received, up);
        servers.apply(ServerCall::Respond {
            request,
            status: 200,
            content_type: String::from("application/octet-stream"),
            headers: Vec::new(),
            body: Vec::new(),
            file: Some(ServedFile {
                path: String::from("f"),
                size: down.len() as u64,
            }),
        });
        while servers.any() && !client.is_finished() {
            servers.pump(|path, at, buffer| {
                assert_eq!(path, "f");
                let at = at as usize;
                let n = buffer.len().min(down.len() - at);
                buffer[..n].copy_from_slice(&down[at..at + n]);
                Some(n)
            });
            if servers.streams.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let answer = client.join().unwrap();
        let split = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let head = String::from_utf8_lossy(&answer[..split]);
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(head.contains(&format!("Content-Length: {}", down.len())));
        assert!(answer[split..] == down[..], "the file arrives whole");
        servers.apply(ServerCall::Stop { server: 1 });
        assert!(!servers.any());
    }
}
