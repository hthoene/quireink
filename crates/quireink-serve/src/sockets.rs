//! Plugins' sockets for shells with the standard library (see
//! `quireink_core::SocketCall`): each TCP connection has a thread that
//! connects and reads what the kernel asks for, and one that writes;
//! broadcasts have a thread that sends and collects the answers. Reads
//! happen only when asked, one at a time per socket, so what waits for
//! the kernel is at most one part per socket.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use quireink_core::{SocketCall, SocketEvent};

/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest answer to a broadcast read.
const DATAGRAM: usize = 1500;

type Wake = Arc<dyn Fn() + Send + Sync>;

/// An open connection, seen from the shell's side.
struct Connection {
    reads: Sender<u32>,
    writes: Sender<Vec<u8>>,
    /// Set once the kernel closed it: nothing is reported after.
    closed: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<TcpStream>>>,
}

/// The sockets and broadcasts of all plugins.
pub struct Sockets {
    events: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    open: HashMap<u32, Connection>,
    broadcasts: usize,
    /// Where broadcasts go; the local network's broadcast address.
    broadcast_to: Ipv4Addr,
    wake: Wake,
}

impl std::fmt::Debug for Sockets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sockets")
            .field("open", &self.open.len())
            .finish_non_exhaustive()
    }
}

impl Sockets {
    /// `wake` is called when there is something to pass to the kernel,
    /// e.g. to wake an event loop.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (events, receiver) = channel();
        Self {
            events,
            receiver,
            open: HashMap::new(),
            broadcasts: 0,
            broadcast_to: Ipv4Addr::BROADCAST,
            wake: Arc::new(wake),
        }
    }

    /// Does what the kernel asks.
    pub fn apply(&mut self, call: SocketCall) {
        match call {
            SocketCall::Connect { socket, host, port } => self.connect(socket, host, port),
            SocketCall::Send { socket, data } => {
                if let Some(c) = self.open.get(&socket) {
                    let _ = c.writes.send(data);
                }
            }
            SocketCall::Read { socket, max } => {
                if let Some(c) = self.open.get(&socket) {
                    let _ = c.reads.send(max);
                }
            }
            SocketCall::Close { socket } => {
                if let Some(c) = self.open.remove(&socket) {
                    c.closed.store(true, Ordering::Relaxed);
                    if let Ok(stream) = c.stream.lock()
                        && let Some(stream) = stream.as_ref()
                    {
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                }
            }
            SocketCall::Broadcast {
                broadcast,
                ports,
                data,
                wait_ms,
            } => self.broadcast(broadcast, ports, data, wait_ms),
        }
    }

    /// An event to pass to the kernel, if any.
    pub fn take_event(&mut self) -> Option<SocketEvent> {
        let event = self.receiver.try_recv().ok()?;
        match &event {
            SocketEvent::Failed { socket, .. } => {
                self.open.remove(socket);
            }
            SocketEvent::Done { .. } => self.broadcasts = self.broadcasts.saturating_sub(1),
            _ => {}
        }
        Some(event)
    }

    /// Whether a socket or broadcast is open, e.g. to keep polling.
    pub fn any(&self) -> bool {
        !self.open.is_empty() || self.broadcasts > 0
    }

    fn connect(&mut self, socket: u32, host: String, port: u16) {
        let (reads, read_asks) = channel::<u32>();
        let (writes, write_asks) = channel::<Vec<u8>>();
        let closed = Arc::new(AtomicBool::new(false));
        let stream = Arc::new(Mutex::new(None));
        self.open.insert(
            socket,
            Connection {
                reads,
                writes,
                closed: closed.clone(),
                stream: stream.clone(),
            },
        );
        let events = self.events.clone();
        let wake = self.wake.clone();
        let report = move |event: SocketEvent| {
            if !closed.load(Ordering::Relaxed) {
                let _ = events.send(event);
                wake();
            }
        };
        thread::spawn(move || {
            let connected = (host.as_str(), port)
                .to_socket_addrs()
                .map_err(|e| e.to_string())
                .and_then(|mut addresses| {
                    addresses.next().ok_or_else(|| String::from("no such host"))
                })
                .and_then(|address: SocketAddr| {
                    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|e| e.to_string())
                });
            let mut reading = match connected {
                Ok(s) => s,
                Err(reason) => return report(SocketEvent::Failed { socket, reason }),
            };
            let _ = reading.set_nodelay(true);
            let writing = match reading.try_clone() {
                Ok(w) => w,
                Err(e) => {
                    return report(SocketEvent::Failed {
                        socket,
                        reason: e.to_string(),
                    });
                }
            };
            if let Ok(mut held) = stream.lock() {
                *held = reading.try_clone().ok();
            }
            report(SocketEvent::Connected { socket });
            let failed = Arc::new(AtomicBool::new(false));
            // Writing on a thread of its own, so a read waiting does not
            // hold sends back.
            {
                let report = report.clone();
                let failed = failed.clone();
                let mut writing = writing;
                thread::spawn(move || {
                    for data in write_asks {
                        if let Err(e) = writing.write_all(&data).and_then(|()| writing.flush()) {
                            if !failed.swap(true, Ordering::Relaxed) {
                                report(SocketEvent::Failed {
                                    socket,
                                    reason: e.to_string(),
                                });
                            }
                            return;
                        }
                    }
                });
            }
            for max in read_asks {
                let mut buffer = vec![0; max.max(1) as usize];
                match reading.read(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        report(SocketEvent::Data {
                            socket,
                            data: buffer,
                        });
                    }
                    Err(e) => {
                        if !failed.swap(true, Ordering::Relaxed) {
                            report(SocketEvent::Failed {
                                socket,
                                reason: e.to_string(),
                            });
                        }
                        return;
                    }
                }
            }
        });
    }

    fn broadcast(&mut self, broadcast: u32, ports: Vec<u16>, data: Vec<u8>, wait_ms: u32) {
        self.broadcasts += 1;
        let events = self.events.clone();
        let wake = self.wake.clone();
        let to = self.broadcast_to;
        thread::spawn(move || {
            let report = |event| {
                let _ = events.send(event);
                wake();
            };
            if let Ok(udp) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
                && udp.set_broadcast(true).is_ok()
            {
                for port in ports {
                    let _ = udp.send_to(&data, (to, port));
                }
                let until = Instant::now() + Duration::from_millis(u64::from(wait_ms));
                let mut buffer = [0u8; DATAGRAM];
                while let Some(left) = until.checked_duration_since(Instant::now()) {
                    if left.is_zero() || udp.set_read_timeout(Some(left)).is_err() {
                        break;
                    }
                    match udp.recv_from(&mut buffer) {
                        Ok((n, from)) => report(SocketEvent::Answer {
                            broadcast,
                            from: from.ip().to_string(),
                            data: buffer[..n].to_vec(),
                        }),
                        Err(_) => break,
                    }
                }
            }
            report(SocketEvent::Done { broadcast });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// The next event, waiting up to two seconds.
    fn wait(sockets: &mut Sockets) -> SocketEvent {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = sockets.take_event() {
                return event;
            }
            assert!(Instant::now() < until, "no event");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn connects_reads_when_asked_and_sends() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.write_all(b"12[9,{}]").unwrap();
            let mut got = [0u8; 5];
            peer.read_exact(&mut got).unwrap();
            got
        });
        let mut sockets = Sockets::new(|| {});
        sockets.apply(SocketCall::Connect {
            socket: 1,
            host: String::from("127.0.0.1"),
            port,
        });
        assert_eq!(wait(&mut sockets), SocketEvent::Connected { socket: 1 });
        assert!(sockets.any());
        sockets.apply(SocketCall::Read { socket: 1, max: 64 });
        let SocketEvent::Data { socket: 1, data } = wait(&mut sockets) else {
            panic!("no data");
        };
        assert!(b"12[9,{}]".starts_with(&data) && !data.is_empty());
        sockets.apply(SocketCall::Send {
            socket: 1,
            data: b"hello".to_vec(),
        });
        assert_eq!(&server.join().unwrap(), b"hello");
        sockets.apply(SocketCall::Close { socket: 1 });
        assert!(!sockets.any());
    }

    #[test]
    fn a_closed_peer_reads_as_nothing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || drop(listener.accept().unwrap()));
        let mut sockets = Sockets::new(|| {});
        sockets.apply(SocketCall::Connect {
            socket: 2,
            host: String::from("127.0.0.1"),
            port,
        });
        assert_eq!(wait(&mut sockets), SocketEvent::Connected { socket: 2 });
        server.join().unwrap();
        sockets.apply(SocketCall::Read { socket: 2, max: 8 });
        match wait(&mut sockets) {
            SocketEvent::Data { data, .. } => assert!(data.is_empty()),
            SocketEvent::Failed { .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unreachable_hosts_fail() {
        let mut sockets = Sockets::new(|| {});
        sockets.apply(SocketCall::Connect {
            socket: 3,
            host: String::from("no-such-host.invalid"),
            port: 9,
        });
        assert!(matches!(
            wait(&mut sockets),
            SocketEvent::Failed { socket: 3, .. }
        ));
        assert!(!sockets.any());
    }

    #[test]
    fn broadcasts_collect_answers() {
        let service = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = service.local_addr().unwrap().port();
        let answering = thread::spawn(move || {
            let mut buffer = [0u8; 64];
            let (n, from) = service.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..n], b"hello");
            service
                .send_to(b"calibre wireless device client (on pc);8080,9090", from)
                .unwrap();
        });
        let mut sockets = Sockets::new(|| {});
        sockets.broadcast_to = Ipv4Addr::LOCALHOST;
        sockets.apply(SocketCall::Broadcast {
            broadcast: 1,
            ports: vec![port],
            data: b"hello".to_vec(),
            wait_ms: 1500,
        });
        let SocketEvent::Answer { from, data, .. } = wait(&mut sockets) else {
            panic!("no answer");
        };
        assert_eq!(from, "127.0.0.1");
        assert!(data.ends_with(b";8080,9090"));
        answering.join().unwrap();
        // Done once the time is up.
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(SocketEvent::Done { broadcast: 1 }) = sockets.take_event() {
                break;
            }
            assert!(Instant::now() < until, "not done");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!sockets.any());
    }
}
