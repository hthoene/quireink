//! A whole session with Calibre as its driver behaves: found by its
//! answer to the broadcast, then initialisation, a book sent, the books
//! listed, the book deleted, the end. Calibre's bytes come as the plugin
//! asks for them, never more.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::rc::Rc;

use quireink_core::{Kernel, SocketCall, SocketEvent};
use quireink_platform::{ButtonState, InputEvent, Rotation};
use quireink_plugin_calibre::Calibre;
use quireink_plugin_calibre::protocol::{self, frame, op};
use quireink_sdk::{
    DeviceInfo, DirEntry, DisplayInfo, InputCaps, Instant, MemoryClass, MemoryStorage, PixelFormat,
    Storage, StorageError,
};

const DEVICE: DeviceInfo = DeviceInfo {
    id: "test",
    name: "Test device",
    display: DisplayInfo {
        width: 300,
        height: 400,
        dpi: 100,
        format: PixelFormat::Gray4,
        rotation: Rotation::Deg0,
        color: None,
    },
    input: InputCaps {
        buttons: true,
        touch: false,
        pen: false,
        keys: &[],
    },
    memory: MemoryClass::Large,
};

/// A storage the test looks into while the kernel has it.
#[derive(Clone)]
struct Shared(Rc<MemoryStorage>);

impl Storage for Shared {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        self.0.read_at(path, offset, buf)
    }
    fn size(&self, path: &str) -> Result<u64, StorageError> {
        self.0.size(path)
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.0.write(path, data)
    }
    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.0.append(path, data)
    }
    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        self.0.list(dir, visit)
    }
    fn remove(&self, path: &str) -> Result<(), StorageError> {
        self.0.remove(path)
    }
    fn exists(&self, path: &str) -> bool {
        self.0.exists(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.0.rename(from, to)
    }
    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        self.0.create_dir(path)
    }
}

/// Calibre's side of the socket: what it still has to send, what came
/// from the device, and the device's read waiting for bytes.
#[derive(Default)]
struct Peer {
    wire: Vec<u8>,
    got: Vec<u8>,
    waiting: Option<(u32, u32)>,
    closed: bool,
}

impl Peer {
    /// Serves the kernel's socket calls: sends are kept, a read is
    /// answered with as much as it asks of what is on the wire.
    fn pump(&mut self, kernel: &mut Kernel) {
        loop {
            for call in kernel.take_socket_calls() {
                match call {
                    SocketCall::Send { data, .. } => self.got.extend_from_slice(&data),
                    SocketCall::Close { .. } => self.closed = true,
                    SocketCall::Read { socket, max } => self.waiting = Some((socket, max)),
                    other => panic!("unexpected {other:?}"),
                }
            }
            match self.waiting {
                Some((socket, max)) if !self.wire.is_empty() => {
                    self.waiting = None;
                    let n = (max as usize).min(self.wire.len());
                    let data: Vec<u8> = self.wire.drain(..n).collect();
                    kernel.socket_event(Instant(5), SocketEvent::Data { socket, data });
                }
                _ => return,
            }
        }
    }

    fn say(&mut self, kernel: &mut Kernel, op: u8, args: &str) {
        self.wire.extend_from_slice(&frame(op, args));
        self.pump(kernel);
    }

    /// The messages the device sent since the last call.
    fn answers(&mut self) -> Vec<protocol::Message> {
        let mut framing = protocol::Framing::default();
        let mut out = Vec::new();
        let text = String::from_utf8(std::mem::take(&mut self.got)).unwrap();
        let mut rest = text.as_str();
        while !rest.is_empty() {
            let n = (framing.wanted() as usize).min(rest.len());
            let (part, tail) = rest.split_at(n);
            rest = tail;
            if let Some(message) = framing.feed(part).unwrap() {
                out.push(message);
            }
        }
        out
    }
}

const CONFIG: &str = "[keymap.global]\nbutton.calibre = calibre.open\n";

fn press(kernel: &mut Kernel, name: &'static str) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        kernel.handle(Instant(1), InputEvent::Button { name, state });
    }
}

fn kernel(storage: &Shared, config: &str) -> Kernel {
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage.clone())
        .user_config(config)
        .network(true)
        .plugin(Calibre)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    press(&mut kernel, "calibre");
    kernel
}

#[test]
fn a_session_with_calibre() {
    let storage = Shared(Rc::new(MemoryStorage::new()));
    let mut kernel = kernel(&storage, CONFIG);

    // Looking for Calibre: a broadcast to its ports, and its answer.
    let calls = kernel.take_socket_calls();
    let [
        SocketCall::Broadcast {
            broadcast, ports, ..
        },
    ] = calls.as_slice()
    else {
        panic!("{calls:?}");
    };
    assert_eq!(ports.as_slice(), protocol::BROADCAST_PORTS);
    kernel.socket_event(
        Instant(2),
        SocketEvent::Answer {
            broadcast: *broadcast,
            from: String::from("192.168.1.5"),
            data: b"calibre wireless device client (on pc);8080,9091".to_vec(),
        },
    );
    let calls = kernel.take_socket_calls();
    let [SocketCall::Connect { socket, host, port }] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!((host.as_str(), *port), ("192.168.1.5", 9091));
    kernel.socket_event(Instant(3), SocketEvent::Connected { socket: *socket });
    let mut calibre = Peer::default();
    calibre.pump(&mut kernel);
    assert!(calibre.waiting.is_some(), "reads once connected");

    // Initialisation: the device says what it can.
    calibre.say(
        &mut kernel,
        op::GET_INITIALIZATION_INFO,
        r#"{"serverProtocolVersion": 1, "validExtensions": ["epub", "mobi"],
            "passwordChallenge": "", "currentLibraryName": "Books"}"#,
    );
    let answers = calibre.answers();
    assert_eq!(answers.len(), 1);
    let info = &answers[0];
    assert_eq!(info.op, op::OK);
    for flag in [
        "versionOK",
        "canStreamBooks",
        "canStreamMetadata",
        "canReceiveBookBinary",
        "canDeleteMultipleBooks",
    ] {
        assert_eq!(
            protocol::text(&info.args, flag).as_deref(),
            Some("true"),
            "{flag}"
        );
    }
    assert!(protocol::number(&info.args, "maxBookContentPacketLen").unwrap() > 0);
    calibre.say(
        &mut kernel,
        op::SET_CALIBRE_DEVICE_INFO,
        r#"{"device_store_uuid": "store-1", "device_name": "QuireInk"}"#,
    );
    calibre.say(&mut kernel, op::GET_DEVICE_INFORMATION, "{}");
    calibre.say(&mut kernel, op::FREE_SPACE, "{}");
    let answers = calibre.answers();
    assert_eq!(answers.len(), 3);
    let device_info = protocol::member(&answers[1].args, "device_info").unwrap();
    assert_eq!(
        protocol::text(device_info, "device_store_uuid").as_deref(),
        Some("store-1")
    );
    assert!(protocol::number(&answers[2].args, "free_space_on_device").unwrap() > 0);

    // A book: its message, then its bytes, of which the device takes
    // exactly as many as Calibre said.
    let book = b"PK\x03\x04 an epub, not text \xff\xfe".to_vec();
    let message = format!(
        r#"{{"lpath": "Ann Author/A Book.epub", "length": {}, "thisBook": 0, "totalBooks": 1,
            "metadata": {{"lpath": "Ann Author/A Book.epub", "uuid": "uuid-a", "title": "A Book"}},
            "willStreamBooks": true, "willStreamBinary": true, "wantsSendOkToSendbook": false}}"#,
        book.len()
    );
    calibre
        .wire
        .extend_from_slice(&frame(op::SEND_BOOK, &message));
    calibre.wire.extend_from_slice(&book);
    calibre.pump(&mut kernel);
    assert!(calibre.answers().is_empty(), "no answer to a book");
    assert!(calibre.wire.is_empty());
    let saved = quireink_sdk::read_file(&storage, "Ann Author/A Book.epub", 1 << 20).unwrap();
    assert_eq!(saved, book);
    assert!(!storage.exists("Ann Author/A Book.epub.part"));

    // The books on the device: the one sent, with its metadata.
    calibre.say(
        &mut kernel,
        op::GET_BOOK_COUNT,
        r#"{"canStream": true, "willUseCachedMetadata": false}"#,
    );
    let answers = calibre.answers();
    assert_eq!(protocol::number(&answers[0].args, "count"), Some(1));
    assert_eq!(
        protocol::text(&answers[1].args, "uuid").as_deref(),
        Some("uuid-a")
    );

    // Deleted from Calibre: the file goes, Calibre hears its uuid.
    calibre.say(
        &mut kernel,
        op::DELETE_BOOK,
        r#"{"lpaths": ["Ann Author/A Book.epub"]}"#,
    );
    let answers = calibre.answers();
    assert_eq!(answers.len(), 2);
    assert_eq!(
        protocol::text(&answers[1].args, "uuid").as_deref(),
        Some("uuid-a")
    );
    assert!(!storage.exists("Ann Author/A Book.epub"));

    // A keep-alive is answered; ejecting ends the connection.
    calibre.say(&mut kernel, op::NOOP, "{}");
    assert_eq!(calibre.answers().len(), 1);
    calibre.say(&mut kernel, op::NOOP, r#"{"ejecting": true}"#);
    assert_eq!(calibre.answers().len(), 1);
    assert!(calibre.closed);
}

#[test]
fn a_path_out_of_the_library_is_dropped() {
    let storage = Shared(Rc::new(MemoryStorage::new()));
    let config = format!("{CONFIG}[calibre]\naddress = 10.0.0.2\n");
    let mut kernel = kernel(&storage, &config);
    let calls = kernel.take_socket_calls();
    let [SocketCall::Connect { socket, host, port }] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!((host.as_str(), *port), ("10.0.0.2", protocol::DEFAULT_PORT));
    kernel.socket_event(Instant(3), SocketEvent::Connected { socket: *socket });
    let mut calibre = Peer::default();
    calibre.wire.extend_from_slice(&frame(
        op::SEND_BOOK,
        r#"{"lpath": "../outside.epub", "length": 3, "metadata": {}}"#,
    ));
    calibre.wire.extend_from_slice(b"abc");
    // The device goes on reading after the dropped book.
    calibre.wire.extend_from_slice(&frame(op::NOOP, "{}"));
    calibre.pump(&mut kernel);
    assert!(!storage.exists("../outside.epub") && !storage.exists("outside.epub"));
    assert!(!storage.exists(".quireink/plugins/calibre/dropped.part"));
    assert_eq!(calibre.answers().len(), 1, "the NOOP after it was answered");
}
