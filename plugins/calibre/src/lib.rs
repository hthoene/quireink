//! Calibre's wireless device connection: while the screen is open, the
//! device finds Calibre in the local network (or connects to the address
//! set), and Calibre sends books to it, lists them and deletes them, as
//! with a reader on a cable. Books go into the library folder, where
//! Calibre places them (`Author/Title.epub`).
//!
//! In Calibre: *Connect/share*, *Start wireless device connection*. A
//! password set there is asked for once and kept with the other passwords.
//!
//! The protocol is in `protocol`, the books Calibre knows in `library`.
//! The connection is a socket (ADR 0014); books come straight into
//! storage, so their size does not matter.

#![no_std]

extern crate alloc;

mod library;
pub mod protocol;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, groups, menus, params, slots};
use quireink_sdk::json;
use quireink_sdk::secrets::SecretChange;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, SocketRead, Version, socket_params as p,
};

use protocol::{Message, op};

pub const PLUGIN: Id = Id::new_static("calibre");
/// Opens the connection screen.
pub const OPEN: Id = Id::new_static("calibre.open");
const SCREEN: Id = Id::new_static("calibre.screen");
const SOCKET: Id = Id::new_static("calibre.socket");
const FIND: Id = Id::new_static("calibre.find");
const EVENTS: Id = Id::new_static("calibre.events");
const PASSWORD_TYPED: Id = Id::new_static("calibre.password-typed");
/// Calibre's address, if it is not found by itself: `host` or `host:port`.
pub const ADDRESS: Id = Id::new_static("calibre.address");
/// The password set in Calibre, kept as a secret.
const PASSWORD: &str = "calibre.password";
/// How long to wait for Calibre's answer to the broadcast.
const FIND_MS: u32 = 3000;
/// The library folder (the `files` plugin's setting).
const LIBRARY: &str = "files.root";
/// Space reported to Calibre: storages do not say how much is free, and
/// Calibre refuses to send to a device it thinks full.
const SPACE: u64 = 64 << 30;

/// What Calibre's metadata of a book says of it, for the library.
fn provided(metadata: &str) -> quireink_sdk::library::Provided {
    let text = |key: &str| json::raw_member(metadata, key).and_then(json::unquote);
    let list = |key: &str| {
        json::raw_member(metadata, key)
            .map(json::string_array)
            .unwrap_or_default()
    };
    quireink_sdk::library::Provided {
        title: text("title"),
        author: list("authors").into_iter().next(),
        series: text("series"),
        series_index: json::raw_member(metadata, "series_index")
            .filter(|n| *n != "null")
            .map(|n| n.trim_end_matches(".0").into()),
        tags: list("tags"),
    }
}
/// The Calibre plugin.
#[derive(Debug, Default)]
pub struct Calibre;

impl Plugin for Calibre {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Calibre"),
            description: Cow::Borrowed(
                "Receive books from Calibre over the network, as its wireless device.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("calibre.title"),
            factory: Box::new(|env, _| Box::new(CalibreScreen::new(env))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("calibre.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 47,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("transfer"))?;
        r.add_setting(
            SettingSpec::new(
                ADDRESS,
                "calibre.setting.address",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("")),
            )
            .group(groups::LIBRARY)
            .description("calibre.setting.address.help")
            .advanced(),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Where the connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Finding,
    Connecting,
    Connected,
    /// A book is coming: its title, which of how many.
    Receiving {
        title: String,
        number: u64,
        total: u64,
    },
    /// Calibre asked for a password, or the one kept was wrong.
    Password,
    /// It ended: why (a translated text).
    Ended(String),
}

/// A book coming: written to `part`, then moved to `path`.
#[derive(Debug)]
struct Coming {
    lpath: String,
    path: String,
    part: String,
    metadata: String,
}

struct CalibreScreen {
    style: UiStyle,
    header: ui::Header,
    state: State,
    framing: protocol::Framing,
    coming: Option<Coming>,
    /// Calibre's name and library, once known.
    name: Option<String>,
    library: Option<String>,
    received: u32,
    started: bool,
    /// The *Type the password* row, where it shows.
    password_row: Option<Rect>,
}

impl CalibreScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        Self {
            header: ui::Header::new(env, &style, true),
            style,
            state: State::Finding,
            framing: protocol::Framing::default(),
            coming: None,
            name: None,
            library: None,
            received: 0,
            started: false,
            password_row: None,
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    /// Finds Calibre, or connects to the address set.
    fn start(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        self.framing = protocol::Framing::default();
        self.coming = None;
        let address = env.settings.text(ADDRESS.as_str()).trim();
        if address.is_empty() {
            self.state = State::Finding;
            cx.command(Command::Broadcast {
                id: FIND,
                ports: protocol::BROADCAST_PORTS.to_vec(),
                data: b"hello".to_vec(),
                wait_ms: FIND_MS,
                events: EVENTS,
            });
        } else {
            let (host, port) = match address.rsplit_once(':') {
                Some((host, port)) if port.parse::<u16>().is_ok() => {
                    (host, port.parse().unwrap_or(protocol::DEFAULT_PORT))
                }
                _ => (address, protocol::DEFAULT_PORT),
            };
            self.connect(host, port, cx);
        }
        self.redraw(cx);
    }

    fn connect(&mut self, host: &str, port: u16, cx: &mut Cx<'_, '_>) {
        self.state = State::Connecting;
        cx.command(Command::Connect {
            socket: SOCKET,
            host: host.to_string(),
            port,
            events: EVENTS,
        });
    }

    fn end(&mut self, text: String, cx: &mut Cx<'_, '_>) {
        self.state = State::Ended(text);
        self.coming = None;
        self.redraw(cx);
    }

    fn send(&self, op: u8, args: &str, cx: &mut Cx<'_, '_>) {
        cx.command(Command::SocketSend {
            socket: SOCKET,
            data: protocol::frame(op, args),
        });
    }

    /// Asks for what comes next of a message.
    fn read_on(&self, cx: &mut Cx<'_, '_>) {
        cx.command(Command::SocketRead {
            socket: SOCKET,
            read: SocketRead::Text {
                max: self.framing.wanted(),
            },
        });
    }

    fn on_socket(&mut self, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let reason = params.get(p::REASON).unwrap_or_default();
        match params.get(p::KIND).unwrap_or_default() {
            p::ANSWER if self.state == State::Finding => {
                let text = params.get(p::TEXT).unwrap_or_default();
                if let Some(port) = protocol::port_of_answer(text) {
                    self.name = protocol::name_of_answer(text).map(String::from);
                    let host = params.get(p::FROM).unwrap_or_default().to_string();
                    self.connect(&host, port, cx);
                    self.redraw(cx);
                }
            }
            p::DONE if self.state == State::Finding => {
                let text = if reason == "no network" {
                    env.i18n.tr("calibre.no-network")
                } else {
                    env.i18n.tr("calibre.not-found")
                };
                self.end(text.to_string(), cx);
            }
            p::CONNECTED => {
                self.state = State::Connected;
                self.read_on(cx);
                self.redraw(cx);
            }
            p::RECEIVED => {
                let text = params.get(p::TEXT).unwrap_or_default();
                if text.is_empty() {
                    let ended = env.i18n.tr("calibre.closed").to_string();
                    return self.end(ended, cx);
                }
                match self.framing.feed(text) {
                    Ok(Some(message)) => self.handle(message, cx),
                    Ok(None) => self.read_on(cx),
                    Err(_) => {
                        cx.command(Command::Disconnect(SOCKET));
                        let ended = env.i18n.tr("calibre.not-calibre").to_string();
                        self.end(ended, cx);
                    }
                }
            }
            p::SAVED => {
                match self.coming.take() {
                    Some(coming) => {
                        let _ = env.storage.remove(&coming.path);
                        if env.storage.rename(&coming.part, &coming.path).is_ok() {
                            library::remember(env.storage, &coming.lpath, &coming.metadata);
                            quireink_sdk::library::provide(
                                env.storage,
                                &coming.path,
                                provided(&coming.metadata),
                            );
                            self.received += 1;
                        }
                    }
                    // A book dropped (its path led out of the library).
                    None => {
                        if let Some(path) = params.get(p::PATH) {
                            let _ = env.storage.remove(path);
                        }
                    }
                }
                self.state = State::Connected;
                self.read_on(cx);
                self.redraw(cx);
            }
            p::CLOSED | p::FAILED if params.get(params::ID) == Some(SOCKET.as_str()) => {
                if let Some(coming) = self.coming.take() {
                    let _ = env.storage.remove(&coming.part);
                }
                if !matches!(self.state, State::Ended(_) | State::Password) {
                    let text = if reason.is_empty() {
                        env.i18n.tr("calibre.closed").to_string()
                    } else {
                        env.i18n
                            .format("calibre.failed", &[("reason", Arg::Str(reason))])
                    };
                    self.end(text, cx);
                }
            }
            _ => {}
        }
    }

    /// Answers a message of Calibre's, then reads on (unless a book or
    /// the end comes).
    fn handle(&mut self, message: Message, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let args = message.args.as_str();
        match message.op {
            op::GET_INITIALIZATION_INFO => {
                let challenge = protocol::text(args, "passwordChallenge").unwrap_or_default();
                let password = env.secrets.get(PASSWORD).map(String::from);
                let extensions: Vec<&str> = env
                    .formats
                    .iter()
                    .flat_map(|f| f.extensions.iter().map(|e| e.as_ref()))
                    .collect();
                let version = format!("{}", SDK_VERSION);
                let name = format!("QuireInk ({})", env.device.name);
                let device = protocol::Device {
                    name: &name,
                    extensions: &extensions,
                    challenge: &challenge,
                    password: password.as_deref(),
                    version: &version,
                };
                self.send(op::OK, &protocol::initialization(&device), cx);
            }
            op::SET_CALIBRE_DEVICE_INFO => {
                library::set_device_info(env.storage, args);
                self.send(op::OK, "{}", cx);
            }
            op::SET_CALIBRE_DEVICE_NAME | op::SET_LIBRARY_INFO => {
                if let Some(name) = protocol::text(args, "libraryName") {
                    self.library = Some(name);
                    self.redraw(cx);
                }
                self.send(op::OK, "{}", cx);
            }
            op::GET_DEVICE_INFORMATION => {
                let stored = library::device_info(env.storage);
                let version = format!("{}", SDK_VERSION);
                let name = format!("QuireInk ({})", env.device.name);
                let info = protocol::device_information(stored.as_deref(), &name, &version);
                self.send(op::OK, &info, cx);
            }
            op::TOTAL_SPACE => {
                self.send(
                    op::OK,
                    &format!("{{\"total_space_on_device\": {SPACE}}}"),
                    cx,
                );
            }
            op::FREE_SPACE => {
                self.send(
                    op::OK,
                    &format!("{{\"free_space_on_device\": {SPACE}}}"),
                    cx,
                );
            }
            op::GET_BOOK_COUNT => {
                let books = library::books(env.storage, library_folder(&env));
                let count = format!(
                    "{{\"count\": {}, \"willStream\": true, \"willScan\": true}}",
                    books.len()
                );
                self.send(op::OK, &count, cx);
                for metadata in books {
                    self.send(op::OK, &metadata, cx);
                }
            }
            op::SEND_BOOK => return self.book(args, cx),
            op::DELETE_BOOK => {
                self.send(op::OK, "{}", cx);
                for lpath in protocol::strings(args, "lpaths") {
                    if let Some(path) = protocol::safe_path(&lpath) {
                        let _ = env
                            .storage
                            .remove(&quireink_sdk::files::join(library_folder(&env), &path));
                    }
                    let uuid = library::forget(env.storage, &lpath).unwrap_or_default();
                    let reply = format!("{{\"uuid\": {}}}", quireink_sdk::json::string(&uuid));
                    self.send(op::OK, &reply, cx);
                }
            }
            op::NOOP => {
                if protocol::member(args, "ejecting").is_some() {
                    self.send(op::OK, "{}", cx);
                    cx.command(Command::Disconnect(SOCKET));
                    let ended = env.i18n.tr("calibre.ejected").to_string();
                    return self.end(ended, cx);
                }
                // Without cached metadata Calibre counts nothing; a plain
                // NOOP keeps the connection alive.
                if protocol::member(args, "count").is_none()
                    && protocol::member(args, "priKey").is_none()
                {
                    self.send(op::OK, "{}", cx);
                }
            }
            op::DISPLAY_MESSAGE => {
                // Kind 1: the password is wrong or missing.
                if protocol::number(args, "messageKind") == Some(1) {
                    cx.command(Command::Disconnect(SOCKET));
                    self.state = State::Password;
                    self.redraw(cx);
                    return;
                }
            }
            op::GET_BOOK_FILE_SEGMENT | op::GET_BOOK_METADATA => {
                let error = "{\"message\": \"QuireInk does not send books\"}";
                self.send(op::ERROR, error, cx);
            }
            op::ERROR => {
                let message = protocol::text(args, "message").unwrap_or_default();
                let text = env
                    .i18n
                    .format("calibre.failed", &[("reason", Arg::Str(&message))]);
                cx.command(Command::Disconnect(SOCKET));
                return self.end(text, cx);
            }
            // Book lists and their metadata, busy, done: nothing to say.
            _ => {}
        }
        self.read_on(cx);
    }

    /// A book is about to come: its bytes go to a part file first.
    fn book(&mut self, args: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let length = protocol::number(args, "length").unwrap_or(0);
        let lpath = protocol::text(args, "lpath").unwrap_or_default();
        let metadata = protocol::member(args, "metadata")
            .unwrap_or("{}")
            .to_string();
        let title = protocol::text(&metadata, "title").unwrap_or_else(|| lpath.clone());
        // A path out of the library: the bytes are taken, then dropped.
        let (path, part) = match protocol::safe_path(&lpath) {
            Some(safe) => {
                let path = quireink_sdk::files::join(library_folder(&env), &safe);
                if let Some(dir) = path.rsplit_once('/').map(|(d, _)| d) {
                    let _ = env.storage.create_dir(dir);
                }
                (path.clone(), format!("{path}.part"))
            }
            None => (
                String::new(),
                format!("{}/dropped.part", Env::plugin_dir(PLUGIN.as_str())),
            ),
        };
        self.state = State::Receiving {
            title,
            number: protocol::number(args, "thisBook").unwrap_or(0) + 1,
            total: protocol::number(args, "totalBooks").unwrap_or(1),
        };
        self.coming = (!path.is_empty()).then(|| Coming {
            lpath,
            path,
            part: part.clone(),
            metadata,
        });
        cx.command(Command::SocketRead {
            socket: SOCKET,
            read: SocketRead::File {
                path: part,
                len: length,
            },
        });
        self.redraw(cx);
    }

    fn type_password(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let request = Params::new()
            .with(params::TITLE, env.i18n.tr("calibre.password").to_string())
            .with(params::MODE, "password")
            .with(params::REPLY, PASSWORD_TYPED.as_str());
        cx.command(Command::OpenSlot(slots::KEYBOARD, request));
    }

    /// The area below the header.
    fn content(&self, env: &Env<'_>) -> Rect {
        let display = env.device.display;
        let top = self.header.area.height + self.style.gap as u16;
        Rect::new(0, top, display.width, display.height.saturating_sub(top))
    }
}

/// The library folder, where Calibre's books go.
fn library_folder<'e>(env: &Env<'e>) -> &'e str {
    env.settings.text(LIBRARY).trim_matches('/')
}

impl Screen for CalibreScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("calibre.title"),
            None,
            None,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        let area = self.content(env);
        let margin = style.margin as f32;
        let width = f32::from(area.width) - 2.0 * margin;
        let mut y = area.y + style.gap as u16;
        let mut line = |canvas: &mut Canvas<'_>, size: f32, text: &str, color: Color| {
            let height = font.line_metrics(size).line_height as u16;
            for range in font.wrap(size, text, width) {
                let row = Rect::new(area.x, y, area.width, height);
                ui::draw_label(canvas, &font, size, margin, row, &text[range], color);
                y += height;
            }
            y += style.gap as u16 / 2;
        };
        let name = self.name.clone().unwrap_or_else(|| String::from("Calibre"));
        let status = match &self.state {
            State::Finding => env.i18n.tr("calibre.finding").to_string(),
            State::Connecting => env.i18n.tr("calibre.connecting").to_string(),
            State::Connected => env
                .i18n
                .format("calibre.connected", &[("name", Arg::Str(&name))]),
            State::Receiving {
                title,
                number,
                total,
            } => env.i18n.format(
                "calibre.receiving",
                &[
                    ("title", Arg::Str(title)),
                    ("number", Arg::Int(*number as i64)),
                    ("total", Arg::Int(*total as i64)),
                ],
            ),
            State::Password => env.i18n.tr("calibre.password-needed").to_string(),
            State::Ended(text) => text.clone(),
        };
        line(canvas, style.title, &status, Color::BLACK);
        if let Some(library) = &self.library {
            let text = env
                .i18n
                .format("calibre.library", &[("library", Arg::Str(library))]);
            line(canvas, style.text, &text, style.faint(80));
        }
        if self.received > 0 {
            let text = env
                .i18n
                .plural("calibre.received", i64::from(self.received), &[]);
            line(canvas, style.text, &text, Color::BLACK);
        }
        let hint = match self.state {
            State::Finding | State::Ended(_) => Some("calibre.hint"),
            State::Connected => Some("calibre.connected-hint"),
            _ => None,
        };
        if let Some(hint) = hint {
            line(canvas, style.small, env.i18n.tr(hint), style.faint(80));
        }
        if matches!(self.state, State::Ended(_)) {
            let again = if env.device.input.touch {
                "calibre.again.touch"
            } else {
                "calibre.again.buttons"
            };
            line(canvas, style.small, env.i18n.tr(again), Color::BLACK);
        }
        self.password_row = None;
        if self.state == State::Password {
            let row = Rect::new(area.x, y, area.width, style.row as u16);
            ui::draw_value_row(
                canvas,
                env,
                &style,
                row,
                false,
                env.i18n.tr("calibre.type-password"),
                "›",
            );
            self.password_row = Some(row);
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if !self.started {
            self.started = true;
            self.start(cx);
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == EVENTS {
            self.on_socket(params, cx);
        } else if *event == PASSWORD_TYPED
            && let Some(password) = params.get(params::TEXT).filter(|t| !t.is_empty())
        {
            cx.command(Command::SetSecret(SecretChange::keep(
                PASSWORD,
                "calibre.password",
                password,
            )));
            self.start(cx);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if *action == actions::CONFIRM {
            match self.state {
                State::Password => self.type_password(cx),
                // Again, after it ended.
                State::Ended(_) => self.start(cx),
                _ => {}
            }
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(action) = self.header.tap(cx.env(), &self.style, gesture.start, None) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        if self.password_row.is_some_and(|r| r.contains(gesture.start)) {
            self.type_password(cx);
            return Handled::Yes;
        }
        if matches!(self.state, State::Ended(_)) {
            self.start(cx);
            return Handled::Yes;
        }
        Handled::No
    }
}

#[cfg(test)]
mod tests;
