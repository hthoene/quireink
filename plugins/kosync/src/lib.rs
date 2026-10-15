//! Reading progress synced with KOReader's sync server ("kosync"), so a
//! book opens where one stopped on another device — QuireInk or KOReader.
//!
//! - Books are identified as KOReader does (a partial MD5 of the file), so
//!   the same file matches on every device.
//! - Positions are KOReader's XPointers where the format has them (EPUB),
//!   besides the percentage.
//! - The progress is sent at most once a minute while reading and when the
//!   app goes to the background; it is fetched when a book opens and when
//!   the app comes back. If another device is further, a dialog offers to
//!   go there (once for each position).
//! - *Sync progress* in the reading menu's *More* does both at once.
//! - The account (settings → *Progress sync*) keeps the user name in the
//!   settings and only the MD5 of the password, as KOReader does. Its
//!   screen also sets the server: KOReader's or any compatible one, also
//!   below a path (`https://example.org/kosync`) or over plain `http` in a
//!   home network.
//!
//! Needs a device with network ([`Env::network`]).

#![no_std]

extern crate alloc;

mod account;
mod confirm;
mod json;

use quireink_sdk::md5;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{events, params};
use quireink_sdk::document::Locator;
use quireink_sdk::{
    ActionSpec, Arg, Command, Env, HttpRequest, Id, Instant, Manifest, MenuEntry, Params, Plugin,
    RegisterError, Registrar, SDK_VERSION, ScreenSpec, SettingGroupSpec, SettingKind, SettingSpec,
    SettingValue, Version, http_params,
};

pub const PLUGIN: Id = Id::new_static("kosync");
/// The account's user name (empty when signed out); edited on the account
/// screen.
pub const USER: Id = Id::new_static("kosync.user");
/// The sync server.
pub const SERVER: Id = Id::new_static("kosync.server");
/// Send and fetch the progress by itself.
pub const AUTO: Id = Id::new_static("kosync.auto");
/// Sends the progress of the open book and fetches the latest now.
pub const SYNC_NOW: Id = Id::new_static("kosync.sync-now");

const GROUP: Id = Id::new_static("kosync.group");
const ACCOUNT_SCREEN: Id = Id::new_static("kosync.account");
const CONFIRM_SCREEN: Id = Id::new_static("kosync.confirm");
/// Emitted by the account screen when one signed in or out.
const ACCOUNT_CHANGED: Id = Id::new_static("kosync.account-changed");
/// Replies of the server.
const PULLED: Id = Id::new_static("kosync.pulled");
const PUSHED: Id = Id::new_static("kosync.pushed");

/// KOReader's public server.
const DEFAULT_SERVER: &str = "https://sync.koreader.rocks";
/// The progress is sent at most this often while reading.
const PUSH_EVERY_MS: u64 = 60_000;
/// The reader is shown again often (after every menu); the progress is
/// fetched again for the same book only after this long.
const PULL_EVERY_MS: u64 = 60_000;
/// Another device counts as further from this much more progress (0..1).
const FURTHER: f64 = 0.002;
/// Parameters of a fetch: the local progress (0..1) and whether the user
/// asked for it.
const LOCAL: &str = "local";
const MANUAL: &str = "manual";

/// Where the reader is in which book.
#[derive(Debug, Clone, PartialEq)]
struct Position {
    path: String,
    at: Locator,
    /// Reading progress from 0 to 1, as the reader shows it.
    progress: f64,
}

#[derive(Debug, Default)]
struct State {
    loaded: bool,
    /// MD5 of the password, from the account file.
    key: Option<String>,
    device_id: String,
    current: Option<Position>,
    /// The position last sent.
    sent: Option<Position>,
    last_push: Option<Instant>,
    /// The book last fetched for, and when.
    last_pull: Option<(String, Instant)>,
    /// A position the user chose not to go to: book and progress.
    declined: Option<(String, String)>,
    /// Book fingerprints already computed.
    digests: Vec<(String, String)>,
}

type Shared = Rc<RefCell<State>>;

fn dir() -> String {
    Env::plugin_dir(PLUGIN.as_str())
}

/// The file with the account's key.
fn key_file() -> String {
    format!("{}/account.ini", dir())
}

fn read_value(env: &Env<'_>, file: &str, key: &str) -> Option<String> {
    let data = quireink_sdk::read_file(env.storage, file, 4096).ok()?;
    let text = core::str::from_utf8(&data).ok()?;
    quireink_sdk::config::parse(text)
        .filter_map(Result::ok)
        .find(|e| e.key == key)
        .map(|e| e.value.as_str().into_owned())
}

/// Saves the key of a signed-in account; `None` signs out.
pub(crate) fn save_key(env: &Env<'_>, key: Option<&str>) {
    match key {
        Some(key) => {
            let text = format!("[account]\nkey = {key}\n");
            let _ = env.storage.write(&key_file(), text.as_bytes());
        }
        None => {
            let _ = env.storage.remove(&key_file());
        }
    }
}

impl State {
    fn load(&mut self, env: &Env<'_>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.key = read_value(env, &key_file(), "key").filter(|k| !k.is_empty());
        if !self.device_id.is_empty() {
            return;
        }
        let device = format!("{}/device.ini", dir());
        self.device_id = match read_value(env, &device, "id") {
            Some(id) => id,
            None => {
                // Different on each installation: the time and where memory
                // happened to be.
                let probe = Box::new(0u8);
                let seed = format!("{}{}{:p}", env.now.millis(), env.device.id, &*probe);
                let id = md5::of(&seed).to_uppercase();
                let text = format!("[device]\nid = {id}\n");
                let _ = env.storage.write(&device, text.as_bytes());
                id
            }
        };
    }

    /// Whether to sync by itself now.
    fn active(&self, env: &Env<'_>) -> bool {
        env.network && env.settings.bool(AUTO.as_str()) && self.signed_in(env)
    }

    fn signed_in(&self, env: &Env<'_>) -> bool {
        !env.settings.text(USER.as_str()).is_empty() && self.key.is_some()
    }

    fn digest(&mut self, env: &Env<'_>, path: &str) -> Option<String> {
        if let Some((_, digest)) = self.digests.iter().find(|(p, _)| p == path) {
            return Some(digest.clone());
        }
        let digest = md5::partial(env.storage, path)?;
        self.digests.push((path.to_string(), digest.clone()));
        Some(digest)
    }

    /// A request to the server with the account's credentials.
    fn request(
        &self,
        env: &Env<'_>,
        method: &str,
        path: &str,
        body: Option<String>,
        reply: Id,
        params: Params,
    ) -> Option<Command> {
        let user = env.settings.text(USER.as_str());
        let key = self.key.as_ref()?;
        let auth = Some((user, key.as_str()));
        Some(Command::Http(server_request(
            env, method, path, body, reply, params, auth,
        )))
    }

    /// Sends the current position, unless it was sent already.
    fn push(&mut self, env: &Env<'_>, out: &mut Vec<Command>, force: bool) {
        let Some(position) = self.current.clone() else {
            return;
        };
        if !force && self.sent.as_ref() == Some(&position) {
            return;
        }
        let Some(digest) = self.digest(env, &position.path) else {
            return;
        };
        let document = env.formats.open(env.storage, &position.path).ok();
        let xpointer = document
            .as_ref()
            .and_then(|d| d.xpointer(env.storage, position.at))
            .unwrap_or_default();
        let progress = document.map_or(position.progress, |d| f64::from(d.progress(position.at)));
        let body = format!(
            "{{\"document\":{},\"progress\":{},\"percentage\":{:.4},\"device\":{},\"device_id\":{}}}",
            json::string(&digest),
            json::string(&xpointer),
            progress,
            json::string(&device_name(env)),
            json::string(&self.device_id),
        );
        let params = Params::new().with(params::PATH, position.path.clone());
        let target = "/syncs/progress";
        if let Some(command) = self.request(env, "PUT", target, Some(body), PUSHED, params) {
            out.push(command);
            self.sent = Some(position);
            self.last_push = Some(env.now);
        }
    }

    /// Asks for the latest position of the book at `path`.
    fn pull(&mut self, env: &Env<'_>, out: &mut Vec<Command>, path: &str, manual: bool) {
        let Some(digest) = self.digest(env, path) else {
            return;
        };
        let local = self
            .current
            .as_ref()
            .filter(|c| c.path == path)
            .map_or(0.0, |c| c.progress);
        let params = Params::new()
            .with(params::PATH, path.to_string())
            .with(LOCAL, format!("{local}"))
            .with(MANUAL, if manual { "1" } else { "0" });
        let target = format!("/syncs/progress/{digest}");
        if let Some(command) = self.request(env, "GET", &target, None, PULLED, params) {
            out.push(command);
            self.last_pull = Some((path.to_string(), env.now));
        }
    }
}

/// How this device shows up on others.
fn device_name(env: &Env<'_>) -> String {
    format!("QuireInk ({})", env.device.name)
}

/// The server's base URL.
pub(crate) fn server(env: &Env<'_>) -> String {
    normalize_server(env.settings.text(SERVER.as_str()))
}

/// A server address as entered, as a base URL: `https://` unless another
/// scheme is given, no `/` at the end, KOReader's server if empty.
pub(crate) fn normalize_server(text: &str) -> String {
    let server = text.trim().trim_end_matches('/');
    if server.is_empty() {
        DEFAULT_SERVER.to_string()
    } else if server.contains("://") {
        server.to_string()
    } else {
        format!("https://{server}")
    }
}

/// A base URL as shown: without `https://` (but with `http://`, which is
/// worth knowing).
pub(crate) fn server_label(url: &str) -> &str {
    url.strip_prefix("https://").unwrap_or(url)
}

/// A request to the sync server, authenticated as user and key (the MD5 of
/// the password) if given.
pub(crate) fn server_request(
    env: &Env<'_>,
    method: &str,
    path: &str,
    body: Option<String>,
    reply: Id,
    params: Params,
    auth: Option<(&str, &str)>,
) -> HttpRequest {
    let mut headers = Vec::from([
        (
            "Accept".to_string(),
            "application/vnd.koreader.v1+json".to_string(),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
    ]);
    if let Some((user, key)) = auth {
        headers.push(("x-auth-user".to_string(), user.to_string()));
        headers.push(("x-auth-key".to_string(), key.to_string()));
    }
    HttpRequest {
        headers,
        body,
        params,
        ..HttpRequest::new(method, &format!("{}{path}", server(env)), reply)
    }
}

/// The position in the parameters of a reading event.
fn position(params: &Params) -> Option<Position> {
    let number = |key| params.get(key).and_then(|v| v.parse::<u32>().ok());
    Some(Position {
        path: params.get(params::PATH)?.to_string(),
        at: Locator::new(number(params::SECTION)?, number(params::OFFSET)?),
        progress: params
            .get(params::PROGRESS)
            .and_then(|p| p.parse::<f64>().ok())
            .map_or(0.0, |p| p / 100.0),
    })
}

/// The translation key telling why a request with HTTP status `status`
/// failed (0: no answer).
pub(crate) fn failure(status: &str) -> &'static str {
    match status {
        "0" => "kosync.unreachable",
        "401" | "403" => "kosync.wrong",
        _ => "kosync.server-error",
    }
}

/// A message about a request in the confirmation overlay.
fn message(env: &Env<'_>, key: &str, status: &str) -> Command {
    let text = env.i18n.format(key, &[("status", Arg::Str(status))]);
    Command::OpenScreen(CONFIRM_SCREEN, Params::new().with(params::TEXT, text))
}

/// Handles the server's answer to a fetch.
fn pulled(state: &State, env: &Env<'_>, params: &Params, out: &mut Vec<Command>) {
    let manual = params.get(MANUAL) == Some("1");
    let path = params.get(params::PATH).unwrap_or_default();
    // The book was closed meanwhile.
    if state.current.as_ref().is_none_or(|c| c.path != path) {
        return;
    }
    let status = params.get(http_params::STATUS).unwrap_or("0");
    if status != "200" {
        if manual {
            out.push(message(env, failure(status), status));
        }
        return;
    }
    let body = json::object(params.get(http_params::BODY).unwrap_or_default());
    let remote = json::number(&body, "percentage").unwrap_or(0.0);
    let local: f64 = params
        .get(LOCAL)
        .and_then(|l| l.parse().ok())
        .unwrap_or(0.0);
    let from_here = json::text(&body, "device_id") == Some(state.device_id.as_str());
    let declined = state
        .declined
        .as_ref()
        .is_some_and(|(p, r)| p == path && *r == format!("{remote}"));
    if !from_here && remote > local + FURTHER && (manual || !declined) {
        // Further on another device: offer to go there.
        let device = json::text(&body, "device").unwrap_or("?");
        let xpointer = json::text(&body, "progress").unwrap_or_default();
        out.push(Command::OpenScreen(
            CONFIRM_SCREEN,
            Params::new()
                .with(params::PATH, path.to_string())
                .with(confirm::XPOINTER, xpointer.to_string())
                .with(confirm::PERCENTAGE, format!("{remote}"))
                .with(confirm::DEVICE, device.to_string()),
        ));
    } else if manual {
        out.push(message(env, "kosync.up-to-date", status));
    }
}

/// The kosync plugin.
#[derive(Debug, Default)]
pub struct Kosync;

impl Plugin for Kosync {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Progress sync"),
            description: Cow::Borrowed(
                "Syncs the reading position with KOReader's sync server, also with KOReader.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let state: Shared = Rc::new(RefCell::new(State::default()));
        r.add_setting_group(SettingGroupSpec {
            id: GROUP,
            title: Cow::Borrowed("kosync.group"),
            order: 45,
        })?;
        for spec in [
            SettingSpec::new(
                USER,
                "kosync.setting.user",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("")),
            )
            .editor(ACCOUNT_SCREEN)
            .description("kosync.setting.user.help"),
            SettingSpec::new(
                AUTO,
                "kosync.setting.auto",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .description("kosync.setting.auto.help"),
            SettingSpec::new(
                SERVER,
                "kosync.setting.server",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed(DEFAULT_SERVER)),
            )
            .advanced()
            .description("kosync.setting.server.help"),
        ] {
            r.add_setting(spec.group(GROUP))?;
        }
        r.add_screen(ScreenSpec {
            id: ACCOUNT_SCREEN,
            title: Cow::Borrowed("kosync.group"),
            factory: Box::new(|env, _| Box::new(account::AccountScreen::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: CONFIRM_SCREEN,
            title: Cow::Borrowed("kosync.group"),
            factory: Box::new(|env, params| Box::new(confirm::ConfirmScreen::new(env, params))),
        })?;

        let s = state.clone();
        r.subscribe(
            events::BOOK_OPENED,
            Box::new(move |env, params, out| {
                let mut state = s.borrow_mut();
                state.load(env);
                let Some(position) = position(params) else {
                    return;
                };
                let active = state.active(env);
                // Another book: the last one's position first.
                let other = state
                    .current
                    .as_ref()
                    .is_some_and(|c| c.path != position.path);
                if active && other {
                    state.push(env, out, false);
                }
                let path = position.path.clone();
                state.current = Some(position);
                let recent = state
                    .last_pull
                    .as_ref()
                    .is_some_and(|(p, at)| *p == path && env.now.since(*at) < PULL_EVERY_MS);
                if active && !recent {
                    state.pull(env, out, &path, false);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::PAGE_TURNED,
            Box::new(move |env, params, out| {
                let mut state = s.borrow_mut();
                state.load(env);
                if let Some(position) = position(params) {
                    state.current = Some(position);
                }
                let due = state
                    .last_push
                    .is_none_or(|at| env.now.since(at) >= PUSH_EVERY_MS);
                if due && state.active(env) {
                    state.push(env, out, false);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::SUSPEND,
            Box::new(move |env, _, out| {
                let mut state = s.borrow_mut();
                state.load(env);
                if state.active(env) {
                    state.push(env, out, false);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::RESUME,
            Box::new(move |env, _, out| {
                // Maybe read on another device meanwhile.
                let mut state = s.borrow_mut();
                state.load(env);
                let path = state.current.as_ref().map(|c| c.path.clone());
                if let Some(path) = path.filter(|_| state.active(env)) {
                    state.pull(env, out, &path, false);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            confirm::DECLINED,
            Box::new(move |_, params, _| {
                let path = params.get(params::PATH).unwrap_or_default();
                let progress = params.get(confirm::PERCENTAGE).unwrap_or_default();
                s.borrow_mut().declined = Some((path.to_string(), progress.to_string()));
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            ACCOUNT_CHANGED,
            Box::new(move |_, _, _| s.borrow_mut().loaded = false),
        )?;
        let s = state.clone();
        r.subscribe(
            PUSHED,
            Box::new(move |_, params, _| {
                // Not sent after all: again with the next page.
                if params.get(http_params::STATUS) != Some("200") {
                    let mut state = s.borrow_mut();
                    state.sent = None;
                    state.last_push = None;
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            PULLED,
            Box::new(move |env, params, out| pulled(&s.borrow(), env, params, out)),
        )?;

        // "Sync progress" in the reading menu's "More".
        let s = state;
        r.add_action(ActionSpec {
            id: SYNC_NOW,
            title: Cow::Borrowed("kosync.sync-now"),
            handler: Some(Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                let mut state = s.borrow_mut();
                state.load(env);
                if !env.network || !state.signed_in(env) {
                    out.push(Command::OpenScreen(ACCOUNT_SCREEN, Params::new()));
                    return;
                }
                let Some(path) = state.current.as_ref().map(|c| c.path.clone()) else {
                    return;
                };
                state.push(env, out, true);
                state.pull(env, out, &path, true);
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: Id::new_static("reader.more"),
            item: SYNC_NOW,
            order: 25,
        })?;
        r.add_icon(SYNC_NOW, Cow::Borrowed("refresh"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    use super::*;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn positions_come_from_reading_events() {
        let params = Params::new()
            .with(params::PATH, "books/a.epub")
            .with(params::PROGRESS, "42.5")
            .with(params::SECTION, "3")
            .with(params::OFFSET, "120");
        let found = position(&params).unwrap();
        assert_eq!(found.at, Locator::new(3, 120));
        assert!((found.progress - 0.425).abs() < 1e-9);
        assert!(position(&Params::new().with(params::PATH, "a")).is_none());
    }

    #[test]
    fn server_addresses_are_completed() {
        assert_eq!(normalize_server(""), DEFAULT_SERVER);
        assert_eq!(
            normalize_server(" sync.example.org/ "),
            "https://sync.example.org"
        );
        assert_eq!(
            normalize_server("http://192.168.1.5:8081"),
            "http://192.168.1.5:8081"
        );
        assert_eq!(
            normalize_server("example.org/kosync"),
            "https://example.org/kosync"
        );
        assert_eq!(
            server_label("https://example.org/kosync"),
            "example.org/kosync"
        );
        assert_eq!(server_label("http://nas:8081"), "http://nas:8081");
    }

    #[test]
    fn failures_have_messages() {
        assert_eq!(failure("0"), "kosync.unreachable");
        assert_eq!(failure("401"), "kosync.wrong");
        assert_eq!(failure("500"), "kosync.server-error");
    }
}
