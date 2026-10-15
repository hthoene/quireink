//! Wallabag: articles saved to read later, fetched as books into the
//! `Wallabag` folder of the library, and archived in Wallabag once read.
//!
//! *Wallabag* in the tools sets up the account (the server, user name
//! and password, and an API client's ID and secret, made in Wallabag
//! under *API clients management*; typing them can be done on a phone
//! too, see the keyboard) and fetches: each unread article comes as an
//! EPUB Wallabag makes, with its pictures. Articles read to the end are
//! archived with the next fetch and their books removed, as are those
//! archived or deleted elsewhere. The password and the client's secret
//! are kept with the other passwords and keys (see
//! `quireink_sdk::secrets`).
//!
//! The list of articles is read as it streams from storage (see
//! `quireink_sdk::json::stream`) and each book is written straight to
//! storage, so this works on small devices too. Shipped switched off:
//! switch it on in the plugins.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{actions, events, groups, menus, params, slots};
use quireink_sdk::json::stream::{self, Event};
use quireink_sdk::secrets::SecretChange;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::web::{clean_name, encode};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, HttpRequest, Id,
    Manifest, MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION,
    Screen, ScreenSpec, SettingKind, SettingSpec, SettingValue, Version, http_params, json,
};

pub const PLUGIN: Id = Id::new_static("wallabag");
/// Opens the set-up.
pub const OPEN: Id = Id::new_static("wallabag.open");
/// Fetches the articles.
pub const FETCH: Id = Id::new_static("wallabag.fetch");
/// The server's address, the user name and the API client's ID.
pub const SERVER: Id = Id::new_static("wallabag.server");
pub const USER: Id = Id::new_static("wallabag.user");
pub const CLIENT: Id = Id::new_static("wallabag.client-id");
/// The password and the API client's secret, among the passwords and
/// keys.
pub const PASSWORD: &str = "wallabag.password";
pub const SECRET: &str = "wallabag.client-secret";
const SCREEN: Id = Id::new_static("wallabag.setup");
/// Answers: the token, an archived article, the list, a book.
const TOKEN: Id = Id::new_static("wallabag.token");
const ARCHIVED: Id = Id::new_static("wallabag.archived");
const LISTED: Id = Id::new_static("wallabag.listed");
const DOWNLOADED: Id = Id::new_static("wallabag.downloaded");
/// Emitted with a message (`text`) when a fetch ends.
const DONE: Id = Id::new_static("wallabag.done");
/// Typed on the keyboard; parameter `field`.
const ENTERED: Id = Id::new_static("wallabag.entered");
const FIELD: &str = "field";
const ARTICLE: &str = "article";
/// The folder chooser of the `files` plugin, if installed.
const LIBRARY: &str = "files.root";
const FOLDER: &str = "Wallabag";
/// Unread articles fetched at most.
const MAX_ARTICLES: usize = 30;
/// Reading progress, in percent, from which an article counts as read
/// where its pages are not counted yet.
const READ: f64 = 95.0;

fn data(name: &str) -> String {
    format!("{}/{name}", Env::plugin_dir(PLUGIN.as_str()))
}

/// The folder of the articles: `Wallabag` in the library.
fn folder(env: &Env<'_>) -> String {
    let library = env.settings.text(LIBRARY).trim_matches('/');
    if library.is_empty() {
        String::from(FOLDER)
    } else {
        format!("{library}/{FOLDER}")
    }
}

/// The server as a base URL: `https://` unless another scheme is given,
/// no `/` at the end.
fn server(env: &Env<'_>) -> String {
    let text = env
        .settings
        .text(SERVER.as_str())
        .trim()
        .trim_end_matches('/');
    if text.is_empty() || text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    }
}

/// An article fetched: its id in Wallabag and its book.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Article {
    id: String,
    path: String,
}

/// The articles fetched, kept as `id<TAB>path` lines.
fn articles(env: &Env<'_>) -> Vec<Article> {
    let Ok(data) = quireink_sdk::read_file(env.storage, &data("articles.txt"), 64 * 1024) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|line| {
            let (id, path) = line.split_once('\t')?;
            Some(Article {
                id: id.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

fn save_articles(env: &Env<'_>, list: &[Article]) {
    let text: String = list
        .iter()
        .map(|a| format!("{}\t{}\n", a.id, a.path))
        .collect();
    let _ = env.storage.write(&data("articles.txt"), text.as_bytes());
}

/// The ids of articles read to the end, to archive.
fn finished(env: &Env<'_>) -> Vec<String> {
    quireink_sdk::read_file(env.storage, &data("finished.txt"), 16 * 1024)
        .map(|d| {
            String::from_utf8_lossy(&d)
                .lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn save_finished(env: &Env<'_>, ids: &[String]) {
    let text: String = ids.iter().map(|id| format!("{id}\n")).collect();
    let _ = env.storage.write(&data("finished.txt"), text.as_bytes());
}

/// Notes an article read to the end, from the reader's page turns.
fn page_turned(env: &Env<'_>, p: &Params) {
    let Some(path) = p.get(params::PATH) else {
        return;
    };
    let number = |key| p.get(key).and_then(|v| v.parse::<f64>().ok());
    // On the last page, or near the end where pages are not counted yet.
    let read = match (number(params::PAGE), number(params::PAGES)) {
        (Some(page), Some(pages)) => page >= pages,
        _ => number(params::PROGRESS).is_some_and(|p| p >= READ),
    };
    if !read || !path.starts_with(&folder(env)) {
        return;
    }
    let Some(article) = articles(env).into_iter().find(|a| a.path == path) else {
        return;
    };
    let mut ids = finished(env);
    if !ids.contains(&article.id) {
        ids.push(article.id);
        save_finished(env, &ids);
    }
}

/// What a fetch in progress knows.
#[derive(Debug, Default)]
struct Fetch {
    running: bool,
    token: String,
    /// Articles to archive, then to download (id and title).
    archive: Vec<String>,
    downloads: Vec<(String, String)>,
    listed: bool,
    archived: usize,
    fetched: usize,
    failed: usize,
}

type Shared = Rc<RefCell<Fetch>>;

impl Fetch {
    fn request(&self, env: &Env<'_>, method: &str, path: &str, reply: Id) -> HttpRequest {
        let mut request = HttpRequest::new(method, &format!("{}{path}", server(env)), reply);
        request.headers.push((
            String::from("Authorization"),
            format!("Bearer {}", self.token),
        ));
        request.headers.push((
            String::from("User-Agent"),
            String::from("QuireInk/0.1 (e-reader)"),
        ));
        request
    }

    /// Starts a fetch: first a token for the account.
    fn start(&mut self, env: &Env<'_>, out: &mut Vec<Command>) {
        if self.running {
            return;
        }
        if !env.network {
            return done(env, "wallabag.no-network", &[], out);
        }
        let (Some(password), Some(secret)) = (env.secrets.get(PASSWORD), env.secrets.get(SECRET))
        else {
            return done(env, "wallabag.setup-needed", &[], out);
        };
        let (user, client) = (
            env.settings.text(USER.as_str()),
            env.settings.text(CLIENT.as_str()),
        );
        if server(env).is_empty() || user.is_empty() || client.is_empty() {
            return done(env, "wallabag.setup-needed", &[], out);
        }
        *self = Self {
            running: true,
            ..Self::default()
        };
        let mut request =
            HttpRequest::new("POST", &format!("{}/oauth/v2/token", server(env)), TOKEN);
        request.headers.push((
            String::from("Content-Type"),
            String::from("application/x-www-form-urlencoded"),
        ));
        request.body = Some(format!(
            "grant_type=password&client_id={}&client_secret={}&username={}&password={}",
            encode(client),
            encode(secret),
            encode(user),
            encode(password)
        ));
        out.push(Command::Notify(
            env.i18n.tr("wallabag.fetching").to_string(),
        ));
        out.push(Command::Http(request));
    }

    fn ok(p: &Params) -> Option<u16> {
        p.get(http_params::STATUS)
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|s| (200..300).contains(s))
    }

    fn token(&mut self, env: &Env<'_>, p: &Params, out: &mut Vec<Command>) {
        let token = Self::ok(p).and_then(|_| {
            let body = p.get(http_params::BODY).unwrap_or_default();
            json::text(&json::object(body), "access_token").map(String::from)
        });
        let Some(token) = token else {
            self.running = false;
            let key = match p.get(http_params::STATUS) {
                Some("0") | None => "wallabag.unreachable",
                _ => "wallabag.bad-login",
            };
            return done(env, key, &[], out);
        };
        self.token = token;
        let fetched = articles(env);
        self.archive = finished(env)
            .into_iter()
            .filter(|id| fetched.iter().any(|a| a.id == *id))
            .collect();
        self.next(env, out);
    }

    /// The next step: archive, list, download, or done.
    fn next(&mut self, env: &Env<'_>, out: &mut Vec<Command>) {
        if let Some(id) = self.archive.pop() {
            let mut request =
                self.request(env, "PATCH", &format!("/api/entries/{id}.json"), ARCHIVED);
            request.headers.push((
                String::from("Content-Type"),
                String::from("application/x-www-form-urlencoded"),
            ));
            request.body = Some(String::from("archive=1"));
            request.params = Params::new().with(ARTICLE, id);
            out.push(Command::Http(request));
            return;
        }
        if !self.listed {
            let path = format!(
                "/api/entries.json?archive=0&detail=metadata&sort=created&order=desc&perPage={MAX_ARTICLES}"
            );
            let mut request = self.request(env, "GET", &path, LISTED);
            request.save_to = Some(data("entries.json"));
            out.push(Command::Http(request));
            return;
        }
        if let Some((id, title)) = self.downloads.pop() {
            let mut request = self.request(
                env,
                "GET",
                &format!("/api/entries/{id}/export.epub"),
                DOWNLOADED,
            );
            request.save_to = Some(data("article.tmp"));
            request.params = Params::new().with(ARTICLE, id).with(params::TITLE, title);
            out.push(Command::Http(request));
            return;
        }
        self.running = false;
        let _ = env.storage.remove(&data("entries.json"));
        let key = if self.failed > 0 {
            "wallabag.done-failed"
        } else {
            "wallabag.done"
        };
        let args = [
            ("fetched", Arg::Int(self.fetched as i64)),
            ("archived", Arg::Int(self.archived as i64)),
            ("failed", Arg::Int(self.failed as i64)),
        ];
        done(env, key, &args, out);
    }

    fn archived(&mut self, env: &Env<'_>, p: &Params, out: &mut Vec<Command>) {
        let id = p.get(ARTICLE).unwrap_or_default();
        if Self::ok(p).is_some() {
            self.archived += 1;
            remove_article(env, id);
        } else {
            self.failed += 1;
        }
        self.next(env, out);
    }

    fn listed(&mut self, env: &Env<'_>, p: &Params, out: &mut Vec<Command>) {
        if Self::ok(p).is_none() {
            self.failed += 1;
            self.downloads.clear();
            self.listed = true;
            return self.next(env, out);
        }
        let unread = unread(env.storage);
        // Those archived or deleted elsewhere go.
        for article in articles(env) {
            if !unread.iter().any(|(id, _)| *id == article.id) {
                remove_article(env, &article.id);
            }
        }
        let fetched = articles(env);
        self.downloads = unread
            .into_iter()
            .filter(|(id, _)| !fetched.iter().any(|a| a.id == *id))
            .collect();
        // The list is newest first: popped from the end, the oldest come
        // first, so the newest are the most recent books.
        self.listed = true;
        self.next(env, out);
    }

    fn downloaded(&mut self, env: &Env<'_>, p: &Params, out: &mut Vec<Command>) {
        let tmp = data("article.tmp");
        let id = p.get(ARTICLE).unwrap_or_default().to_string();
        let title = p.get(params::TITLE).unwrap_or_default();
        let dir = folder(env);
        let _ = env.storage.create_dir(&dir);
        let name = clean_name(title, 80).unwrap_or_else(|| id.clone());
        let mut path = format!("{dir}/{name}.epub");
        if env.storage.exists(&path) {
            path = format!("{dir}/{name} ({id}).epub");
        }
        if Self::ok(p).is_some() && env.storage.rename(&tmp, &path).is_ok() {
            self.fetched += 1;
            let mut list = articles(env);
            list.push(Article { id, path });
            save_articles(env, &list);
        } else {
            self.failed += 1;
            let _ = env.storage.remove(&tmp);
        }
        self.next(env, out);
    }
}

/// The unread articles in the list Wallabag sent: ids and titles.
fn unread(storage: &dyn quireink_sdk::Storage) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = Vec::new();
    let item = ["_embedded", "items", "[]"];
    stream::read_file(storage, &data("entries.json"), &mut |event| match event {
        Event::Open(path) if path == item => items.push((String::new(), String::new())),
        Event::Value(path, value) if path.len() == 4 && path[..3] == item => {
            if let Some(last) = items.last_mut() {
                match path[3].as_str() {
                    "id" => last.0 = value.to_string(),
                    "title" => last.1 = value.to_string(),
                    _ => {}
                }
            }
        }
        _ => {}
    });
    items.retain(|(id, _)| !id.is_empty());
    items.truncate(MAX_ARTICLES);
    items
}

/// Removes an article's book and what is noted about it.
fn remove_article(env: &Env<'_>, id: &str) {
    let mut list = articles(env);
    if let Some(at) = list.iter().position(|a| a.id == id) {
        let article = list.remove(at);
        let _ = env.storage.remove(&article.path);
        save_articles(env, &list);
    }
    let mut ids = finished(env);
    if let Some(at) = ids.iter().position(|f| f == id) {
        ids.remove(at);
        save_finished(env, &ids);
    }
}

/// Ends a fetch with message `key`.
fn done(env: &Env<'_>, key: &str, args: &[(&str, Arg<'_>)], out: &mut Vec<Command>) {
    let text = env.i18n.format(key, args);
    out.push(Command::Notify(text.clone()));
    out.push(Command::Emit(DONE, Params::new().with(params::TEXT, text)));
}

/// The Wallabag plugin.
#[derive(Debug, Default)]
pub struct Wallabag;

impl Plugin for Wallabag {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Wallabag"),
            description: Cow::Borrowed("Articles saved in Wallabag, fetched as books."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let state: Shared = Rc::default();
        for (id, title) in [
            (SERVER, "wallabag.server"),
            (USER, "wallabag.user"),
            (CLIENT, "wallabag.client-id"),
        ] {
            let mut spec = SettingSpec::new(
                id.clone(),
                title,
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("")),
            )
            .group(groups::LIBRARY)
            .editor(SCREEN);
            if id == SERVER {
                spec = spec.description("wallabag.server.help");
            } else {
                spec = spec.advanced();
            }
            r.add_setting(spec)?;
        }
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("wallabag.title"),
            factory: Box::new(|env, _| Box::new(SetupScreen::new(env))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("wallabag.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        let s = state.clone();
        r.add_action(ActionSpec {
            id: FETCH,
            title: Cow::Borrowed("wallabag.fetch"),
            handler: Some(Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                s.borrow_mut().start(env, out);
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 48,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("bookmarks"))?;
        r.add_icon(FETCH, Cow::Borrowed("refresh"))?;
        type Step = fn(&mut Fetch, &Env<'_>, &Params, &mut Vec<Command>);
        let steps: [(Id, Step); 4] = [
            (TOKEN, Fetch::token),
            (ARCHIVED, Fetch::archived),
            (LISTED, Fetch::listed),
            (DOWNLOADED, Fetch::downloaded),
        ];
        for (event, step) in steps {
            let s = state.clone();
            r.subscribe(
                event,
                Box::new(move |env: &Env<'_>, p: &Params, out: &mut Vec<Command>| {
                    step(&mut s.borrow_mut(), env, p, out);
                }),
            )?;
        }
        r.subscribe(
            events::PAGE_TURNED,
            Box::new(|env: &Env<'_>, p: &Params, _: &mut Vec<Command>| page_turned(env, p)),
        )?;
        r.subscribe(
            ENTERED,
            Box::new(|_: &Env<'_>, p: &Params, out: &mut Vec<Command>| entered(p, out)),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// A row of the set-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Server,
    User,
    Password,
    Client,
    Secret,
    Fetch,
}

const ROWS: [Row; 6] = [
    Row::Server,
    Row::User,
    Row::Password,
    Row::Client,
    Row::Secret,
    Row::Fetch,
];

impl Row {
    fn title(self) -> &'static str {
        match self {
            Row::Server => "wallabag.server",
            Row::User => "wallabag.user",
            Row::Password => "wallabag.password",
            Row::Client => "wallabag.client-id",
            Row::Secret => "wallabag.client-secret",
            Row::Fetch => "wallabag.fetch",
        }
    }

    fn field(self) -> &'static str {
        match self {
            Row::Server => "server",
            Row::User => "user",
            Row::Password => "password",
            Row::Client => "client",
            Row::Secret => "secret",
            Row::Fetch => "",
        }
    }

    fn of(field: &str) -> Option<Self> {
        ROWS.into_iter()
            .find(|r| r.field() == field && !field.is_empty())
    }
}

/// Keeps what was typed for a row of the set-up.
fn entered(p: &Params, out: &mut Vec<Command>) {
    let Some(row) = p.get(FIELD).and_then(Row::of) else {
        return;
    };
    let text = p.get(params::TEXT).unwrap_or_default().trim();
    let setting =
        |id: Id| Command::SetSetting(id, SettingValue::Text(Cow::Owned(text.to_string())));
    let secret = |id: &str, label: &str| {
        Command::SetSecret(if text.is_empty() {
            SecretChange::remove(id)
        } else {
            SecretChange::keep(id, label, text)
        })
    };
    out.push(match row {
        Row::Server => setting(SERVER),
        Row::User => setting(USER),
        Row::Client => setting(CLIENT),
        Row::Password => secret(PASSWORD, "wallabag.secret.password"),
        Row::Secret => secret(SECRET, "wallabag.secret.client"),
        Row::Fetch => return,
    });
}

/// The set-up: the account's details and *Fetch articles*, with a note
/// below on how to make an API client, or how the last fetch went.
struct SetupScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    note: Rect,
    message: String,
}

impl SetupScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let height = style.row as u16 * ROWS.len() as u16;
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
            message: env.i18n.tr("wallabag.help").to_string(),
            style,
            header,
        }
    }

    fn activate(&mut self, row: Row, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if row == Row::Fetch {
            cx.command(Command::RunAction(FETCH));
            return;
        }
        let (text, password) = match row {
            Row::Server => (env.settings.text(SERVER.as_str()), false),
            Row::User => (env.settings.text(USER.as_str()), false),
            Row::Client => (env.settings.text(CLIENT.as_str()), false),
            _ => ("", true),
        };
        let mut request = Params::new()
            .with(params::TITLE, env.i18n.tr(row.title()).to_string())
            .with(params::TEXT, text.to_string())
            .with(params::REPLY, ENTERED.as_str())
            .with(FIELD, row.field());
        if password {
            request = request.with(params::MODE, "password");
        }
        cx.command(Command::OpenSlot(slots::KEYBOARD, request));
    }
}

impl Screen for SetupScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let title = env.i18n.tr("wallabag.title");
        self.header.draw(canvas, env, &style, title, None, None);
        self.list.draw(
            canvas,
            &style,
            ROWS.len(),
            |canvas, index, rect, focused| {
                let row = ROWS[index];
                let kept = |id: &str| {
                    if env.secrets.get(id).is_some() {
                        "••••••"
                    } else {
                        ""
                    }
                };
                let value = match row {
                    Row::Server => {
                        let url = server(env);
                        url.strip_prefix("https://").unwrap_or(&url).to_string()
                    }
                    Row::User => env.settings.text(USER.as_str()).to_string(),
                    Row::Client => env.settings.text(CLIENT.as_str()).to_string(),
                    Row::Password => kept(PASSWORD).to_string(),
                    Row::Secret => kept(SECRET).to_string(),
                    Row::Fetch => String::from("›"),
                };
                let title = env.i18n.tr(row.title());
                ui::draw_value_row(canvas, env, &style, rect, focused, title, &value);
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
            self.activate(ROWS[self.list.focus.min(ROWS.len() - 1)], cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, ROWS.len()) != ListChange::None {
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
        let Some(index) = self.list.hit(gesture.start, ROWS.len()) else {
            return Handled::No;
        };
        self.list.focus = index;
        cx.request_redraw_area(self.list.area);
        self.activate(ROWS[index], cx);
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, p: &Params, cx: &mut Cx<'_, '_>) {
        if *event == DONE {
            self.message = p.get(params::TEXT).unwrap_or_default().to_string();
        } else if *event == ENTERED {
            // On to the next row.
            if let Some(row) = p.get(FIELD).and_then(Row::of) {
                let at = ROWS.iter().position(|r| *r == row).unwrap_or(0);
                self.list.focus = (at + 1).min(ROWS.len() - 1);
            }
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
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn reads_the_unread_articles() {
        let list = r#"{"page": 1, "_embedded": {"items": [
            {"id": 7, "title": "Slow \"news\"", "tags": [{"id": 1, "title": "tag"}]},
            {"title": "Second", "id": 9, "_links": {"self": {"href": "/api/entries/9"}}}
        ]}, "_links": {"self": {"href": "x"}}}"#;
        let storage = quireink_sdk::MemoryStorage::new().with(&data("entries.json"), list);
        assert_eq!(
            unread(&storage),
            [
                (String::from("7"), String::from("Slow \"news\"")),
                (String::from("9"), String::from("Second")),
            ]
        );
    }

    #[test]
    fn rows_by_field() {
        for row in ROWS {
            if row != Row::Fetch {
                assert_eq!(Row::of(row.field()), Some(row));
            }
        }
        assert_eq!(Row::of(""), None);
    }
}
