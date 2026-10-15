//! Sharing statistics with KOReader: through the same cloud folder
//! (WebDAV, e.g. Nextcloud) KOReader syncs its statistics to, and by
//! taking over the statistics of KOReader on the same device.
//!
//! A sync downloads `statistics.sqlite3` from the folder, merges it (see
//! [`merge`]), uploads the result (only if nobody changed the file in
//! between: `If-Match` with its ETag, else it starts over) and keeps
//! what it uploaded, to tell later what was deleted. KOReader does the
//! same, so either may sync first.

use alloc::borrow::Cow;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, HttpRequest, Id, Params, Rect,
    RefreshMode, Screen, SettingValue, http_params,
};

use quireink_sdk::secrets::SecretChange;

use crate::merge::{self, Outcome};

/// The folder (its address) and the user; the password is kept with the
/// other passwords and keys (see `quireink_sdk::secrets`) as [`SECRET`].
pub const URL: Id = Id::new_static("stats.webdav-url");
pub const USER: Id = Id::new_static("stats.webdav-user");
pub(crate) const SCREEN: Id = Id::new_static("stats.sync");
/// The password.
pub(crate) const SECRET: &str = "stats.webdav";

const DOWNLOADED: Id = Id::new_static("stats.downloaded");
const UPLOADED: Id = Id::new_static("stats.uploaded");
/// Emitted with a message (`text`) when a sync or import ends.
pub(crate) const DONE: Id = Id::new_static("stats.sync-done");
const URL_ENTERED: Id = Id::new_static("stats.url-entered");
const USER_ENTERED: Id = Id::new_static("stats.user-entered");
const PASSWORD_ENTERED: Id = Id::new_static("stats.password-entered");

/// The file's name in the folder, as KOReader names it.
const FILE: &str = "statistics.sqlite3";
/// Tries when the file changes during a sync.
const ATTEMPTS: u8 = 3;
/// Where KOReader keeps its statistics on a device: on Android's shared
/// storage, and at the root of a memory card.
const KOREADER_FILES: [&str; 2] = [
    "storage/koreader/settings/statistics.sqlite3",
    "koreader/settings/statistics.sqlite3",
];

fn dir() -> String {
    Env::plugin_dir(crate::PLUGIN.as_str())
}

fn file(name: &str) -> String {
    format!("{}/{name}", dir())
}

/// What a sync in progress knows.
#[derive(Debug, Default)]
pub(crate) struct Sync {
    attempt: u8,
    running: bool,
}

pub(crate) type SharedSync = Rc<RefCell<Sync>>;

/// The folder as a base URL: `https://` unless another scheme is given,
/// no `/` at the end.
pub(crate) fn folder(text: &str) -> String {
    let text = text.trim().trim_end_matches('/');
    if text.is_empty() || text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    }
}

fn password<'a>(env: &Env<'a>) -> &'a str {
    env.secrets.get(SECRET).unwrap_or_default()
}

/// Keeps the password, or removes it if empty.
fn save_password(password: &str) -> Command {
    let password = password.trim();
    Command::SetSecret(if password.is_empty() {
        SecretChange::remove(SECRET)
    } else {
        SecretChange::keep(SECRET, "stats.secret", password)
    })
}

/// Moves a password kept in the plugin's folder by earlier versions to
/// the other passwords and keys.
pub(crate) fn move_password(env: &Env<'_>, out: &mut Vec<Command>) {
    let path = file("webdav.ini");
    let Ok(data) = quireink_sdk::read_file(env.storage, &path, 4096) else {
        return;
    };
    let text = core::str::from_utf8(&data).unwrap_or_default();
    let old = quireink_sdk::config::parse(text)
        .filter_map(Result::ok)
        .find(|e| e.key == "password")
        .map(|e| e.value.as_str().into_owned())
        .unwrap_or_default();
    if !old.is_empty() && env.secrets.get(SECRET).is_none() {
        out.push(save_password(&old));
    }
    let _ = env.storage.remove(&path);
}

fn request(env: &Env<'_>, method: &str, reply: Id) -> HttpRequest {
    let url = format!("{}/{FILE}", folder(env.settings.text(URL.as_str())));
    let user = env.settings.text(USER.as_str());
    let mut request = HttpRequest::new(method, &url, reply);
    if !user.is_empty() {
        request.headers.push((
            String::from("Authorization"),
            quireink_sdk::web::basic_auth(user, password(env)),
        ));
    }
    request
}

fn done(out: &mut Vec<Command>, message: String) {
    out.push(Command::Emit(
        DONE,
        Params::new().with(params::TEXT, message),
    ));
}

impl Sync {
    /// Starts a sync, or says why it cannot.
    pub(crate) fn start(&mut self, env: &Env<'_>, out: &mut Vec<Command>) {
        if !env.network {
            return done(out, env.i18n.tr("stats.sync.no-network").to_string());
        }
        if env.settings.text(URL.as_str()).trim().is_empty() {
            return done(out, env.i18n.tr("stats.sync.no-folder").to_string());
        }
        self.running = true;
        self.attempt += 1;
        let mut get = request(env, "GET", DOWNLOADED);
        get.save_to = Some(file("incoming.sqlite3"));
        out.push(Command::Http(get));
    }

    /// The other side's file is here (or not): merge, then upload.
    pub(crate) fn downloaded(&mut self, env: &Env<'_>, params: &Params, out: &mut Vec<Command>) {
        if !self.running {
            return;
        }
        let status = params.get(http_params::STATUS).unwrap_or("0");
        let income = match status {
            "200" => Some(file("incoming.sqlite3")),
            // None yet: this is the first.
            "404" => None,
            _ => return self.fail(env, status, out),
        };
        let cached = file(&format!("{FILE}.sync"));
        let cached = env.storage.exists(&cached).then_some(cached);
        let upload = file(FILE);
        let result = merge::merge(
            env.storage,
            income.as_deref(),
            cached.as_deref(),
            Some(&upload),
            &dir(),
        );
        if let Some(income) = &income {
            let _ = env.storage.remove(income);
        }
        if let Err(e) = result {
            self.running = false;
            self.attempt = 0;
            let reason = format!("{e}");
            let text = env
                .i18n
                .format("stats.sync.failed", &[("reason", Arg::Str(&reason))]);
            return done(out, text);
        }
        let mut put = request(env, "PUT", UPLOADED);
        put.body_from = Some(upload);
        if let Some(etag) = params.get(http_params::ETAG) {
            // A weak validator does not match strongly (as KOReader does).
            let etag = etag.trim_start_matches("W/").trim_start_matches("w/");
            put.headers
                .push((String::from("If-Match"), etag.to_string()));
        }
        put.headers.push((
            String::from("Content-Type"),
            String::from("application/octet-stream"),
        ));
        out.push(Command::Http(put));
    }

    /// The merged file is up (or someone was faster).
    pub(crate) fn uploaded(&mut self, env: &Env<'_>, params: &Params, out: &mut Vec<Command>) {
        if !self.running {
            return;
        }
        let status = params.get(http_params::STATUS).unwrap_or("0");
        match status.parse::<u16>().unwrap_or(0) {
            200..=299 => {
                // What was uploaded is what the next sync compares with.
                let upload = file(FILE);
                let cached = file(&format!("{FILE}.sync"));
                if env.storage.rename(&upload, &cached).is_err()
                    && let Ok(data) = quireink_sdk::read_file(env.storage, &upload, 256 << 20)
                {
                    let _ = env.storage.write(&cached, &data);
                }
                self.running = false;
                self.attempt = 0;
                let books = crate::store::books(env.storage).len();
                let text = env.i18n.format(
                    "stats.sync.done",
                    &[(
                        "books",
                        Arg::Str(&env.i18n.plural("stats.books-count", books as i64, &[])),
                    )],
                );
                done(out, text);
            }
            // Changed in between: again, from the download.
            412 if self.attempt < ATTEMPTS => {
                self.running = false;
                self.start(env, out);
            }
            _ => self.fail(env, status, out),
        }
    }

    fn fail(&mut self, env: &Env<'_>, status: &str, out: &mut Vec<Command>) {
        self.running = false;
        self.attempt = 0;
        let key = match status {
            "0" => "stats.sync.unreachable",
            "401" | "403" => "stats.sync.denied",
            "404" | "409" => "stats.sync.no-such-folder",
            _ => "stats.sync.server-error",
        };
        done(out, env.i18n.format(key, &[("status", Arg::Str(status))]));
    }
}

/// KOReader's statistics on this device, if any.
pub(crate) fn koreader_file(env: &Env<'_>) -> Option<&'static str> {
    KOREADER_FILES.into_iter().find(|f| env.storage.exists(f))
}

/// Takes over KOReader's statistics on this device.
fn import(env: &Env<'_>) -> String {
    let Some(path) = koreader_file(env) else {
        return env.i18n.tr("stats.import.none").to_string();
    };
    // Written by KOReader while it runs: its newest pages may still be
    // in the log beside the file.
    let wal = format!("{path}-wal");
    if env.storage.size(&wal).is_ok_and(|s| s > 0) {
        return env.i18n.tr("stats.import.busy").to_string();
    }
    match merge::merge(env.storage, Some(path), None, None, &dir()) {
        Ok(Outcome { books, stays }) => env.i18n.format(
            "stats.import.done",
            &[
                (
                    "books",
                    Arg::Str(&env.i18n.plural("stats.books-count", books as i64, &[])),
                ),
                ("stays", Arg::Int(stays as i64)),
            ],
        ),
        Err(e) => {
            let reason = format!("{e}");
            env.i18n
                .format("stats.sync.failed", &[("reason", Arg::Str(&reason))])
        }
    }
}

/// Registers the sync's settings, events and screen.
pub(crate) fn register(
    r: &mut dyn quireink_sdk::Registrar,
    sync: SharedSync,
) -> Result<(), quireink_sdk::RegisterError> {
    use quireink_sdk::{ScreenSpec, SettingKind, SettingSpec};
    r.subscribe(
        quireink_sdk::builtin::events::STARTED,
        alloc::boxed::Box::new(|env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
            move_password(env, out)
        }),
    )?;
    r.add_setting(
        SettingSpec::new(
            URL,
            "stats.setting.sync",
            SettingKind::Text,
            SettingValue::Text(Cow::Borrowed("")),
        )
        .editor(SCREEN)
        .group(crate::GROUP)
        .description("stats.setting.sync.help"),
    )?;
    r.add_setting(
        SettingSpec::new(
            USER,
            "stats.sync.user",
            SettingKind::Text,
            SettingValue::Text(Cow::Borrowed("")),
        )
        .group(crate::GROUP)
        .editor(SCREEN)
        .advanced(),
    )?;
    let s = sync.clone();
    r.subscribe(
        DOWNLOADED,
        alloc::boxed::Box::new(move |env, params, out| s.borrow_mut().downloaded(env, params, out)),
    )?;
    let s = sync.clone();
    r.subscribe(
        UPLOADED,
        alloc::boxed::Box::new(move |env, params, out| s.borrow_mut().uploaded(env, params, out)),
    )?;
    r.add_screen(ScreenSpec {
        id: SCREEN,
        title: Cow::Borrowed("stats.sync.title"),
        factory: alloc::boxed::Box::new(move |env, _| {
            alloc::boxed::Box::new(SyncScreen::new(env, sync.clone()))
        }),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Folder,
    User,
    Password,
    SyncNow,
    Import,
}

/// The sync's screen: folder, user and password, *Sync now*, and
/// *Take over from KOReader* when KOReader's statistics are here.
pub(crate) struct SyncScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    note: Rect,
    rows: Vec<Row>,
    sync: SharedSync,
    message: String,
}

impl SyncScreen {
    fn new(env: &Env<'_>, sync: SharedSync) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let mut rows = Vec::from([Row::Folder, Row::User, Row::Password, Row::SyncNow]);
        if koreader_file(env).is_some() {
            rows.push(Row::Import);
        }
        let top = header.area.height + style.gap as u16;
        let height = style.row as u16 * rows.len() as u16;
        let note_top = top + height + style.gap as u16;
        Self {
            list: ListView::new(Rect::new(0, top, display.width, height), style.row)
                .focus_on_keys(env),
            note: Rect::new(
                0,
                note_top,
                display.width,
                display.height.saturating_sub(note_top),
            ),
            rows,
            sync,
            message: env.i18n.tr("stats.sync.help").to_string(),
            style,
            header,
        }
    }

    fn show(&mut self, message: String, cx: &mut Cx<'_, '_>) {
        self.message = message;
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn activate(&mut self, row: Row, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let keyboard = |title: &str, text: String, reply: Id, mode: Option<&str>| {
            let mut request = Params::new()
                .with(params::TITLE, env.i18n.tr(title).to_string())
                .with(params::TEXT, text)
                .with(params::REPLY, reply.as_str());
            if let Some(mode) = mode {
                request = request.with(params::MODE, mode.to_string());
            }
            Command::OpenSlot(slots::KEYBOARD, request)
        };
        match row {
            Row::Folder => cx.command(keyboard(
                "stats.sync.folder",
                env.settings.text(URL.as_str()).to_string(),
                URL_ENTERED,
                None,
            )),
            Row::User => cx.command(keyboard(
                "stats.sync.user",
                env.settings.text(USER.as_str()).to_string(),
                USER_ENTERED,
                None,
            )),
            Row::Password => cx.command(keyboard(
                "stats.sync.password",
                String::new(),
                PASSWORD_ENTERED,
                Some("password"),
            )),
            Row::SyncNow => {
                if self.sync.borrow().running {
                    return;
                }
                let mut commands = Vec::new();
                self.sync.borrow_mut().start(&env, &mut commands);
                for command in commands {
                    cx.command(command);
                }
                if self.sync.borrow().running {
                    self.show(env.i18n.tr("stats.sync.running").to_string(), cx);
                }
            }
            Row::Import => {
                let message = import(&env);
                self.show(message, cx);
            }
        }
    }
}

impl Screen for SyncScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let title = env.i18n.tr("stats.sync.title");
        self.header.draw(canvas, env, &style, title, None, None);
        let rows = &self.rows;
        let has_password = !password(env).is_empty();
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| {
                let (title, value) = match rows[index] {
                    Row::Folder => {
                        let url = folder(env.settings.text(URL.as_str()));
                        let shown = url.strip_prefix("https://").unwrap_or(&url).to_string();
                        ("stats.sync.folder", shown)
                    }
                    Row::User => (
                        "stats.sync.user",
                        env.settings.text(USER.as_str()).to_string(),
                    ),
                    Row::Password => (
                        "stats.sync.password",
                        String::from(if has_password {
                            "••••••"
                        } else {
                            ""
                        }),
                    ),
                    Row::SyncNow => ("stats.sync.now", String::from("›")),
                    Row::Import => ("stats.import", String::from("›")),
                };
                ui::draw_value_row(
                    canvas,
                    env,
                    &style,
                    rect,
                    focused,
                    env.i18n.tr(title),
                    &value,
                );
            },
        );
        ui::draw_message(canvas, env, &style, self.note, &self.message);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::CONFIRM {
            self.activate(self.rows[self.list.focus.min(self.rows.len() - 1)], cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, self.rows.len()) != ListChange::None {
            cx.request_redraw_area(self.list.area);
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(action) = self.header.tap(cx.env(), &self.style, gesture.start, None) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, self.rows.len()) else {
            return Handled::No;
        };
        self.list.focus = index;
        cx.request_redraw_area(self.list.area);
        self.activate(self.rows[index], cx);
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let text = params.get(params::TEXT).unwrap_or_default();
        if *event == URL_ENTERED {
            let value = SettingValue::Text(Cow::Owned(folder(text)));
            cx.command(Command::SetSetting(URL, value));
            self.list.focus = 1;
        } else if *event == USER_ENTERED {
            let value = SettingValue::Text(Cow::Owned(text.trim().to_string()));
            cx.command(Command::SetSetting(USER, value));
            self.list.focus = 2;
        } else if *event == PASSWORD_ENTERED {
            cx.command(save_password(text));
            self.list.focus = 3;
        } else if *event == DONE {
            return self.show(text.to_string(), cx);
        } else {
            return;
        }
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_folders() {
        assert_eq!(
            folder(" cloud.example.org/dav/KOReader/ "),
            "https://cloud.example.org/dav/KOReader"
        );
        assert_eq!(folder("http://nas:8080/dav"), "http://nas:8080/dav");
    }
}
