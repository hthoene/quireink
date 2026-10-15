//! A simple browser for reading the web and fetching books and fonts.
//! Off until switched on in the plugin manager.
//!
//! - The start screen offers *Search or enter address*, the bookmarks and
//!   the pages read lately. Holding an entry (with buttons: a long press
//!   on confirm) offers what can be done with it.
//! - A web page opens in the reader, which shows its reading matter like
//!   a book (see the HTML format). Its links lead to further pages;
//!   *Back* returns to the page before, and from the first one to the
//!   start screen.
//! - Books (EPUB, text) go to the library, fonts to the fonts folder
//!   (usable after a restart), anything else to `Downloads`.
//!
//! Bound to the slot [`slots::BROWSER`], so the reader opens links out of
//! books here; another browser plugin can take its place.

#![no_std]

extern crate alloc;

mod store;
mod web;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{actions, menus, params, slots};
use quireink_sdk::files;
use quireink_sdk::ui::choice::{self, Choice};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, HttpRequest,
    Id, Manifest, MenuEntry, Params, Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar,
    SDK_VERSION, Screen, ScreenSpec, Version, http_params,
};

use crate::store::Entry;
use crate::web::Kind;

pub const PLUGIN: Id = Id::new_static("browser");
/// Opens the browser's start screen.
pub const OPEN: Id = Id::new_static("browser.open");
const SCREEN: Id = Id::new_static("browser.screen");
const CHOICE: Id = Id::new_static("browser.choice");
/// Replies: the address typed, a page loaded, a choice made.
const ADDRESS: Id = Id::new_static("browser.address");
const LOADED: Id = Id::new_static("browser.loaded");
const CHOSEN: Id = Id::new_static("browser.chosen");
/// Translation keys of the choices: this, then the option.
const OPTION_LABELS: &str = "browser.option.";

/// The library folder setting of the standard library.
const LIBRARY: &str = "files.root";
/// Where other downloads go.
const DOWNLOADS: &str = "Downloads";

/// The pages shown this time, the one in the reader last: *Back* goes
/// through them.
#[derive(Debug, Default)]
struct Session {
    pages: Vec<Entry>,
}

/// The browser plugin.
#[derive(Debug, Default)]
pub struct Browser;

impl Plugin for Browser {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Browser"),
            description: Cow::Borrowed(
                "Reads web pages like books and fetches books and fonts from the web.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let session = Rc::new(RefCell::new(Session::default()));
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("browser.title"),
            factory: Box::new(move |env, params| {
                Box::new(BrowserScreen::new(env, params, session.clone()))
            }),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("browser.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.bind_slot(slots::BROWSER, SCREEN, Priority::DEFAULT)?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("browser.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenSlot(slots::BROWSER, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::MAIN,
            item: OPEN,
            order: 47,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("world"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// A row of the start screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Address,
    Bookmark(usize),
    History(usize),
}

struct BrowserScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// The line for messages at the bottom.
    status: Rect,
    bookmarks: Vec<Entry>,
    history: Vec<Entry>,
    session: Rc<RefCell<Session>>,
    /// An address to load once shown (opened for a link).
    start: Option<String>,
    /// A page was opened in the reader from here: back here means *Back*.
    showing: bool,
    /// The address being loaded.
    loading: Option<String>,
    message: Option<String>,
}

impl BrowserScreen {
    fn new(env: &Env<'_>, params: &Params, session: Rc<RefCell<Session>>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let line = style.row as u16;
        let status = Rect::new(0, display.height - line, display.width, line);
        let area = Rect::new(
            0,
            top,
            display.width,
            display.height.saturating_sub(top + line),
        );
        let start = params.get(params::URL).map(ToString::to_string);
        if start.is_none() {
            // Opened from the menu: a new start.
            session.borrow_mut().pages.clear();
        }
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            status,
            bookmarks: store::bookmarks(env),
            history: store::load(env.storage, &store::history_path()).unwrap_or_default(),
            session,
            start,
            showing: false,
            loading: None,
            message: None,
            style,
            header,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::from([Row::Address]);
        rows.extend((0..self.bookmarks.len()).map(Row::Bookmark));
        rows.extend((0..self.history.len()).map(Row::History));
        rows
    }

    fn entry(&self, row: Row) -> Option<&Entry> {
        match row {
            Row::Address => None,
            Row::Bookmark(i) => self.bookmarks.get(i),
            Row::History(i) => self.history.get(i),
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    fn say(&mut self, message: String, cx: &mut Cx<'_, '_>) {
        self.message = Some(message);
        self.redraw(cx);
    }

    /// Asks for an address or words to search for.
    fn ask(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        cx.command(Command::OpenSlot(
            slots::KEYBOARD,
            Params::new()
                .with(params::TITLE, env.i18n.tr("browser.address").to_string())
                .with(params::REPLY, ADDRESS.as_str()),
        ));
    }

    /// Loads `url`; what it is decides where it goes once it is there.
    fn load(&mut self, url: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let host = web::host(url).to_string();
        if !env.network {
            let message = env.i18n.tr("browser.no-network").to_string();
            return self.say(message, cx);
        }
        self.loading = Some(url.to_string());
        let message = env
            .i18n
            .format("browser.loading", &[("host", Arg::Str(&host))]);
        self.say(message, cx);
        cx.command(Command::Http(HttpRequest {
            headers: Vec::from([
                (
                    String::from("User-Agent"),
                    String::from("QuireInk/0.1 (e-reader)"),
                ),
                (
                    String::from("Accept"),
                    String::from("text/html,application/xhtml+xml,application/epub+zip,*/*"),
                ),
            ]),
            save_to: Some(download_path()),
            params: Params::new().with(params::URL, url.to_string()),
            ..HttpRequest::new("GET", url, LOADED)
        }));
    }

    /// Shows page `entry` (already loaded) in the reader.
    fn show(&mut self, entry: Entry, cx: &mut Cx<'_, '_>) {
        let path = store::page_path(&entry.url);
        self.history = store::remember(cx.env().storage, entry.clone());
        self.session.borrow_mut().pages.push(entry);
        self.showing = true;
        self.message = None;
        cx.command(Command::OpenSlot(
            slots::READER,
            Params::new().with(params::PATH, path),
        ));
    }

    /// An answer arrived for `url`.
    fn loaded(&mut self, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let requested = params.get(params::URL).unwrap_or_default().to_string();
        if self.loading.as_deref() != Some(requested.as_str()) {
            return;
        }
        self.loading = None;
        let url = params
            .get(http_params::FINAL_URL)
            .map_or(requested.clone(), ToString::to_string);
        let host = web::host(&url).to_string();
        let status: u16 = params
            .get(http_params::STATUS)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let tmp = download_path();
        if status == 0 {
            let message = env
                .i18n
                .format("browser.failed", &[("host", Arg::Str(&host))]);
            return self.say(message, cx);
        }
        let mut head = [0u8; 512];
        let n = env.storage.read_at(&tmp, 0, &mut head).unwrap_or(0);
        let kind = web::kind(params.get(http_params::CONTENT_TYPE), &url, &head[..n]);
        // Error pages are shown like pages; other errors are told.
        if status >= 400 && kind != Kind::Page {
            let _ = env.storage.remove(&tmp);
            let message = env.i18n.format(
                "browser.error",
                &[
                    ("host", Arg::Str(&host)),
                    ("status", Arg::Int(i64::from(status))),
                ],
            );
            return self.say(message, cx);
        }
        match kind {
            Kind::Page => {
                let path = store::page_path(&url);
                if web::keep_page(env.storage, &tmp, &path, &url).is_err() {
                    let message = env.i18n.tr("browser.cannot-save").to_string();
                    return self.say(message, cx);
                }
                let title = web::title(env.storage, &path).unwrap_or(host);
                self.show(Entry::new(&title, &url), cx);
            }
            Kind::Book => {
                let dir = env.settings.text(LIBRARY).trim_matches('/').to_string();
                self.save(&tmp, &dir, &url, "browser.saved-book", true, cx);
            }
            Kind::Font => match env.font_dirs.first() {
                Some(dir) => {
                    let dir = dir.clone();
                    self.save(&tmp, &dir, &url, "browser.saved-font", false, cx);
                }
                None => {
                    let _ = env.storage.remove(&tmp);
                    let message = env.i18n.tr("browser.no-fonts").to_string();
                    self.say(message, cx);
                }
            },
            Kind::Other => self.save(&tmp, DOWNLOADS, &url, "browser.saved-file", false, cx),
        }
    }

    /// Moves a download into `dir` and tells so; books can be read at
    /// once.
    fn save(
        &mut self,
        tmp: &str,
        dir: &str,
        url: &str,
        message: &str,
        book: bool,
        cx: &mut Cx<'_, '_>,
    ) {
        let env = *cx.env();
        let _ = env.storage.create_dir(dir);
        let name = web::file_name(url);
        let target = files::free_path(env.storage, dir, &name);
        if files::move_to(env.storage, tmp, &target).is_err() {
            let _ = env.storage.remove(tmp);
            let message = env.i18n.tr("browser.cannot-save").to_string();
            return self.say(message, cx);
        }
        // Books by their author and title, not the server's file name.
        let target = if book {
            named_book(&env, &target).unwrap_or(target)
        } else {
            target
        };
        let name = files::name(&target).to_string();
        let text = env.i18n.format(
            message,
            &[("name", Arg::Str(&name)), ("folder", Arg::Str(dir))],
        );
        if book && env.formats.supports(&target) {
            self.message = None;
            cx.command(choice::open(
                CHOICE,
                CHOSEN,
                &text,
                &format!("read:{target}"),
                OPTION_LABELS,
                &["read", "ok"],
            ));
            self.redraw(cx);
        } else {
            self.say(text, cx);
        }
    }

    /// Opens what row `index` stands for.
    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        match self.rows().get(index).copied() {
            Some(Row::Address) => self.ask(cx),
            Some(row) => {
                if let Some(url) = self.entry(row).map(|e| e.url.clone()) {
                    self.session.borrow_mut().pages.clear();
                    self.load(&url, cx);
                }
            }
            None => {}
        }
    }

    /// What can be done with row `index`.
    fn offer(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let row = self.rows().get(index).copied();
        let (context, options): (String, &[&str]) = match row {
            Some(Row::Bookmark(i)) => (format!("bookmark:{i}"), &["open", "delete"]),
            Some(Row::History(i)) => (format!("history:{i}"), &["open", "bookmark", "delete"]),
            _ => return self.activate(index, cx),
        };
        let title = row
            .and_then(|row| self.entry(row))
            .map(|e| e.title.clone())
            .unwrap_or_default();
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &title,
            &context,
            OPTION_LABELS,
            options,
        ));
    }

    fn chosen(&mut self, context: &str, option: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some((list, rest)) = context.split_once(':') else {
            return;
        };
        if list == "read" {
            if option == "read" {
                cx.command(Command::OpenSlot(
                    slots::READER,
                    Params::new().with(params::PATH, rest.to_string()),
                ));
            }
            return;
        }
        let Ok(i) = rest.parse::<usize>() else {
            return;
        };
        let entry = match list {
            "bookmark" => self.bookmarks.get(i).cloned(),
            _ => self.history.get(i).cloned(),
        };
        let Some(entry) = entry else {
            return;
        };
        match (list, option) {
            (_, "open") => {
                self.session.borrow_mut().pages.clear();
                self.load(&entry.url, cx);
            }
            ("history", "bookmark") => {
                self.bookmarks.retain(|b| b.url != entry.url);
                self.bookmarks.push(entry);
                store::save(env.storage, &store::bookmarks_path(), &self.bookmarks);
                self.redraw(cx);
            }
            ("bookmark", "delete") => {
                self.bookmarks.remove(i);
                store::save(env.storage, &store::bookmarks_path(), &self.bookmarks);
                self.redraw(cx);
            }
            ("history", "delete") => {
                self.history.remove(i);
                let _ = env.storage.remove(&store::page_path(&entry.url));
                store::save(env.storage, &store::history_path(), &self.history);
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        let len = self.rows().len();
        match self.list.move_focus(delta, len) {
            ListChange::None => {}
            ListChange::Page => {
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
            _ => {
                cx.request_redraw_area(self.list.area);
                cx.request_refresh(RefreshMode::Fast);
            }
        }
    }
}

/// Renames the book at `path` after its author and title; returns where
/// it is then.
fn named_book(env: &Env<'_>, path: &str) -> Option<String> {
    let document = env.formats.open(env.storage, path).ok()?;
    let metadata = document.metadata();
    let title = metadata.title.as_deref()?.trim();
    let name = match metadata.authors.first() {
        Some(author) if !author.trim().is_empty() => format!("{} - {title}", author.trim()),
        _ => title.to_string(),
    };
    drop(document);
    let extension = path.rsplit_once('.').map_or("", |(_, ext)| ext);
    let name = web::clean_name(&name, 100)?;
    let dir = files::parent(path);
    if files::name(path) == format!("{name}.{extension}") {
        return None;
    }
    let target = files::free_path(env.storage, dir, &format!("{name}.{extension}"));
    files::move_to(env.storage, path, &target).ok()?;
    Some(target)
}

/// Where answers are downloaded to before they go where they belong.
fn download_path() -> String {
    format!("{}/download.tmp", store::dir())
}

impl Screen for BrowserScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("browser.title"),
            None,
            self.list.paging(rows.len()),
        );
        let (bookmarks, history) = (&self.bookmarks, &self.history);
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| {
                let (title, value) = match rows[index] {
                    Row::Address => (
                        env.i18n.tr("browser.address").to_string(),
                        String::from("›"),
                    ),
                    Row::Bookmark(i) => (
                        bookmarks[i].title.clone(),
                        web::host(&bookmarks[i].url).to_string(),
                    ),
                    Row::History(i) => (
                        history[i].title.clone(),
                        web::host(&history[i].url).to_string(),
                    ),
                };
                ui::draw_value_row(canvas, env, &style, rect, focused, &title, &value);
            },
        );
        if let (Some(message), Some(font)) = (&self.message, style.font(env)) {
            ui::draw_label(
                canvas,
                &font,
                style.small,
                style.margin as f32,
                self.status,
                message,
                Color::gray(60),
            );
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if let Some(url) = self.start.take() {
            return self.load(&url, cx);
        }
        if !core::mem::take(&mut self.showing) {
            return;
        }
        // Back from a page: the one before, else the start screen.
        let previous = {
            let mut session = self.session.borrow_mut();
            session.pages.pop();
            session.pages.pop()
        };
        let env = *cx.env();
        match previous {
            Some(page) if env.storage.exists(&store::page_path(&page.url)) => self.show(page, cx),
            Some(page) => self.load(&page.url, cx),
            None => {
                self.history = store::load(env.storage, &store::history_path()).unwrap_or_default();
                self.redraw(cx);
            }
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            self.move_focus(-1, cx);
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            self.move_focus(1, cx);
        } else if *a == actions::NEXT_PAGE || *a == actions::PREV_PAGE {
            let rows = self.list.rows() as isize;
            self.move_focus(
                if *a == actions::NEXT_PAGE {
                    rows
                } else {
                    -rows
                },
                cx,
            );
        } else if *a == actions::CONFIRM {
            self.activate(self.list.focus, cx);
        } else if *a == actions::MENU {
            self.offer(self.list.focus, cx);
        } else if *a == actions::BACK {
            cx.command(Command::Back);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows().len();
        let paging = self.list.paging(len);
        if gesture.kind == GestureKind::Tap
            && let Some(action) = self
                .header
                .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, len) else {
            return Handled::No;
        };
        self.list.focus = index;
        cx.request_redraw_area(self.list.area);
        match gesture.kind {
            GestureKind::Tap => self.activate(index, cx),
            GestureKind::LongPress => self.offer(index, cx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == ADDRESS {
            let text = params.get(params::TEXT).unwrap_or_default();
            if !text.trim().is_empty() {
                self.session.borrow_mut().pages.clear();
                let url = web::address(text);
                self.load(&url, cx);
            }
        } else if *event == LOADED {
            self.loaded(params, cx);
        } else if *event == CHOSEN {
            let context = params.get(params::PATH).unwrap_or_default().to_string();
            let option = params.get(choice::OPTION).unwrap_or_default().to_string();
            self.chosen(&context, &option, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}
