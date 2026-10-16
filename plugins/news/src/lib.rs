//! News: feeds (RSS or Atom) fetched as books to read, one a feed, in the
//! `News` folder of the library.
//!
//! *News* in the main menu lists the feeds: *Fetch the news* fetches all
//! of them, a feed is opened with a tap, and holding it offers to fetch
//! it alone or to remove it; *Add a feed* asks for its address (which
//! can come from a phone too, see the keyboard). Each feed becomes a page
//! with its newest articles as plain text (see `convert`), replaced by
//! the next fetch. Feeds are fetched one after the other and each is
//! read as it streams from storage, so this works on small devices too.

#![no_std]

extern crate alloc;

mod convert;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, groups, menus, params, slots};
use quireink_sdk::ui::choice::{self, Choice};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::web::{clean_name, host};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, HttpRequest, Id,
    Manifest, MemoryClass, MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar,
    SDK_VERSION, Screen, ScreenSpec, SettingKind, SettingSpec, SettingValue, Storage, Version,
    http_params,
};

pub const PLUGIN: Id = Id::new_static("news");
/// Opens the list of feeds.
pub const OPEN: Id = Id::new_static("news.open");
/// Fetches all feeds.
pub const FETCH: Id = Id::new_static("news.fetch");
/// Articles kept of each feed.
pub const ARTICLES: Id = Id::new_static("news.articles");
const SCREEN: Id = Id::new_static("news.list");
const CHOICE: Id = Id::new_static("news.choice");
const CHOSEN: Id = Id::new_static("news.chosen");
/// A feed's answer.
const FETCHED: Id = Id::new_static("news.fetched");
/// The address of a new feed, from the keyboard.
const ADDED: Id = Id::new_static("news.added");
const OPTION_LABELS: &str = "news.option.";
/// The folder chooser of the `files` plugin, if installed.
const LIBRARY: &str = "files.root";
const FOLDER: &str = "News";
/// Parameters of a fetch: the feed being fetched, the last to fetch,
/// and how many were fetched and failed so far.
const INDEX: &str = "index";
const LAST: &str = "last";
const DONE: &str = "done";
const FAILED: &str = "failed";
/// Text kept of an article: little on small devices.
const MAX_TEXT_SMALL: usize = 8 * 1024;
const MAX_TEXT: usize = 64 * 1024;

fn data(name: &str) -> String {
    format!("{}/{name}", Env::plugin_dir(PLUGIN.as_str()))
}

/// A feed: its address and its name (the feed's own title once fetched).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Feed {
    url: String,
    name: String,
}

impl Feed {
    fn shown(&self) -> String {
        if self.name.is_empty() {
            host(&self.url).trim_start_matches("www.").to_string()
        } else {
            self.name.clone()
        }
    }

    /// The book it becomes.
    fn book(&self, env: &Env<'_>) -> String {
        let name = clean_name(&self.shown(), 60).unwrap_or_else(|| String::from("News"));
        format!("{}/{name}.html", folder(env))
    }
}

/// A feed's title without its tagline ("Site - all the news" gives "Site").
fn short_title(title: &str) -> String {
    for separator in [" - ", " | ", " \u{2013} ", " \u{2014} ", ": "] {
        if let Some((name, _)) = title.split_once(separator)
            && name.trim().chars().count() >= 3
        {
            return name.trim().to_string();
        }
    }
    title.trim().to_string()
}

/// The folder of the news: `News` in the library.
fn folder(env: &Env<'_>) -> String {
    let library = env.settings.text(LIBRARY).trim_matches('/');
    if library.is_empty() {
        String::from(FOLDER)
    } else {
        format!("{library}/{FOLDER}")
    }
}

fn load(storage: &dyn Storage) -> Vec<Feed> {
    let Ok(data) = quireink_sdk::read_file(storage, &data("feeds.txt"), 64 * 1024) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|line| {
            let (url, name) = line.split_once('\t').unwrap_or((line, ""));
            (!url.trim().is_empty()).then(|| Feed {
                url: url.trim().to_string(),
                name: name.trim().to_string(),
            })
        })
        .collect()
}

fn save(storage: &dyn Storage, feeds: &[Feed]) {
    let text: String = feeds
        .iter()
        .map(|f| format!("{}\t{}\n", f.url, f.name.replace(['\t', '\n'], " ")))
        .collect();
    let _ = storage.write(&data("feeds.txt"), text.as_bytes());
}

/// Asks for feed `index` (of `last`, the last one to fetch).
fn request(feed: &Feed, index: usize, last: usize, (done, failed): (usize, usize)) -> Command {
    let mut request = HttpRequest::new("GET", &feed.url, FETCHED);
    request.headers.push((
        String::from("User-Agent"),
        String::from("QuireInk/0.1 (e-reader)"),
    ));
    request.save_to = Some(data("feed.tmp"));
    request.params = Params::new()
        .with(INDEX, format!("{index}"))
        .with(LAST, format!("{last}"))
        .with(DONE, format!("{done}"))
        .with(FAILED, format!("{failed}"));
    Command::Http(request)
}

/// Starts fetching feeds `from..=last`.
fn fetch(env: &Env<'_>, from: usize, last: usize, out: &mut Vec<Command>) {
    let feeds = load(env.storage);
    if !env.network {
        out.push(Command::Notify(env.i18n.tr("news.no-network").to_string()));
        return;
    }
    match feeds.get(from) {
        Some(feed) if from <= last => {
            out.push(Command::Notify(env.i18n.tr("news.fetching").to_string()));
            out.push(request(feed, from, last.min(feeds.len() - 1), (0, 0)));
        }
        _ => out.push(Command::Notify(env.i18n.tr("news.none").to_string())),
    }
}

/// A feed answered: made into its book, then the next one.
fn fetched(env: &Env<'_>, params: &Params, out: &mut Vec<Command>) {
    let number = |key| params.get(key).and_then(|n| n.parse::<usize>().ok());
    let (Some(index), Some(last)) = (number(INDEX), number(LAST)) else {
        return;
    };
    let mut done = number(DONE).unwrap_or(0);
    let mut failed = number(FAILED).unwrap_or(0);
    let mut feeds = load(env.storage);
    let ok = params
        .get(http_params::STATUS)
        .and_then(|s| s.parse::<u16>().ok())
        .is_some_and(|s| (200..300).contains(&s));
    let made = feeds.get(index).filter(|_| ok).and_then(|feed| {
        let max_text = if env.device.memory == MemoryClass::Small {
            MAX_TEXT_SMALL
        } else {
            MAX_TEXT
        };
        let articles = env.settings.int(ARTICLES.as_str()).clamp(1, 200) as usize;
        let _ = env.storage.create_dir(&folder(env));
        let tmp = data("page.tmp");
        let converted =
            convert::convert(env.storage, &data("feed.tmp"), &tmp, articles, max_text).ok()?;
        let mut feed = feed.clone();
        if feed.name.is_empty() && !converted.title.is_empty() {
            feed.name = short_title(&converted.title);
        }
        env.storage.rename(&tmp, &feed.book(env)).ok()?;
        keep_headlines(env, &feed.book(env), &converted.headlines);
        Some(feed)
    });
    match made {
        Some(feed) => {
            done += 1;
            if feeds[index] != feed {
                feeds[index] = feed;
                save(env.storage, &feeds);
            }
        }
        None => failed += 1,
    }
    let _ = env.storage.remove(&data("feed.tmp"));
    if index < last
        && let Some(next) = feeds.get(index + 1)
    {
        out.push(request(next, index + 1, last, (done, failed)));
        return;
    }
    let message = if failed == 0 {
        env.i18n.plural("news.fetched", done as i64, &[])
    } else {
        env.i18n.plural(
            "news.fetched-failed",
            failed as i64,
            &[("done", Arg::Int(done as i64))],
        )
    };
    out.push(Command::Notify(message));
}

/// Where the newest headlines are kept: `title<TAB>book` lines, the
/// feed fetched last first.
fn headlines_file() -> String {
    data("headlines.txt")
}

/// Keeps `titles` as the headlines of `book`, before the others'.
fn keep_headlines(env: &Env<'_>, book: &str, titles: &[String]) {
    let mut lines: Vec<String> = titles
        .iter()
        .map(|t| format!("{}\t{book}", t.replace(['\t', '\n'], " ")))
        .collect();
    if let Ok(old) = quireink_sdk::read_file(env.storage, &headlines_file(), 32 * 1024) {
        for line in String::from_utf8_lossy(&old).lines() {
            if !line.ends_with(&format!("\t{book}")) && lines.len() < 40 {
                lines.push(line.to_string());
            }
        }
    }
    let _ = env
        .storage
        .write(&headlines_file(), lines.join("\n").as_bytes());
}

/// The newest headlines and their books, at most `max`.
fn headlines(env: &Env<'_>, max: usize) -> Vec<(String, String)> {
    let Ok(data) = quireink_sdk::read_file(env.storage, &headlines_file(), 32 * 1024) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(_, book)| env.storage.exists(book))
        .take(max)
        .map(|(t, b)| (t.to_string(), b.to_string()))
        .collect()
}

/// Headlines the widget shows.
const SHOWN_HEADLINES: usize = 4;

fn headlines_height(env: &Env<'_>, _: u16) -> u16 {
    let count = headlines(env, SHOWN_HEADLINES).len();
    if count == 0 {
        return 0;
    }
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return 0;
    };
    let small = font.line_metrics(style.small).line_height;
    let text = font.line_metrics(style.text).line_height;
    (small + text * count as f32) as u16
}

/// The rows of the headlines below the label.
fn headline_rows(env: &Env<'_>, area: Rect) -> Vec<(Rect, String, String)> {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return Vec::new();
    };
    let small = font.line_metrics(style.small).line_height as u16;
    let text = font.line_metrics(style.text).line_height as u16;
    headlines(env, SHOWN_HEADLINES)
        .into_iter()
        .enumerate()
        .map(|(i, (title, book))| {
            let y = area.y + small + i as u16 * text;
            (Rect::new(area.x, y, area.width, text), title, book)
        })
        .collect()
}

fn draw_headlines(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let small = font.line_metrics(style.small).line_height as u16;
    ui::draw_label(
        canvas,
        &font,
        style.small,
        f32::from(area.x),
        Rect::new(area.x, area.y, area.width, small),
        env.i18n.tr("news.title"),
        style.faint(80),
    );
    for (row, title, _) in headline_rows(env, area) {
        let title = ui::fit(&font, style.text, &title, f32::from(row.width));
        ui::draw_label(
            canvas,
            &font,
            style.text,
            f32::from(row.x),
            row,
            &title,
            quireink_sdk::Color::BLACK,
        );
    }
}

/// The news plugin.
#[derive(Debug, Default)]
pub struct News;

impl Plugin for News {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("News"),
            description: Cow::Borrowed("Fetches news feeds (RSS, Atom) as books to read."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_setting(
            SettingSpec::new(
                ARTICLES,
                "news.setting.articles",
                SettingKind::Int { min: 5, max: 200 },
                SettingValue::Int(30),
            )
            .group(groups::LIBRARY)
            .step(5)
            .advanced()
            .description("news.setting.articles.help"),
        )?;
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("news.title"),
            factory: Box::new(|env, _| Box::new(FeedsScreen::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("news.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("news.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_action(ActionSpec {
            id: FETCH,
            title: Cow::Borrowed("news.fetch"),
            handler: Some(Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                fetch(env, 0, usize::MAX, out);
            })),
        })?;
        r.subscribe(FETCHED, Box::new(fetched))?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 44,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("rss"))?;
        r.add_icon(FETCH, Cow::Borrowed("refresh"))?;
        r.add_widget(quireink_sdk::widgets::WidgetSpec {
            id: Id::new_static("news.widget.headlines"),
            title: Cow::Borrowed("news.widget.headlines"),
            height: Box::new(headlines_height),
            draw: Box::new(draw_headlines),
            signature: Some(Box::new(|env| {
                headlines(env, SHOWN_HEADLINES)
                    .into_iter()
                    .map(|(t, _)| t)
                    .collect::<Vec<_>>()
                    .join("|")
            })),
            targets: Some(Box::new(|env, area| {
                headline_rows(env, area)
                    .into_iter()
                    .map(|(row, _, book)| quireink_sdk::widgets::WidgetTarget {
                        area: row,
                        command: Command::OpenSlot(
                            slots::READER,
                            Params::new().with(params::PATH, book),
                        ),
                    })
                    .collect()
            })),
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// The feeds: *Fetch the news* first, the feeds, then *Add a feed*.
struct FeedsScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    feeds: Vec<Feed>,
}

impl FeedsScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            style,
            header,
            feeds: load(env.storage),
        }
    }

    fn len(&self) -> usize {
        self.feeds.len() + 2
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    fn open(&mut self, row: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if row == 0 {
            fetch_into(&env, 0, usize::MAX, cx);
            return;
        }
        if row == self.feeds.len() + 1 {
            cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                Params::new()
                    .with(params::TITLE, env.i18n.tr("news.address").to_string())
                    .with(params::TEXT, "https://")
                    .with(params::REPLY, ADDED.as_str()),
            ));
            return;
        }
        let index = row - 1;
        let Some(feed) = self.feeds.get(index) else {
            return;
        };
        let book = feed.book(&env);
        if env.storage.exists(&book) {
            cx.command(Command::OpenSlot(
                slots::READER,
                Params::new().with(params::PATH, book),
            ));
        } else {
            fetch_into(&env, index, index, cx);
        }
    }

    fn offer(&mut self, row: usize, cx: &mut Cx<'_, '_>) {
        let Some(feed) = row.checked_sub(1).and_then(|i| self.feeds.get(i)) else {
            return;
        };
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &feed.shown(),
            &format!("{}", row - 1),
            OPTION_LABELS,
            &["open", "fetch", "remove"],
        ));
    }
}

/// The commands of a fetch, given to `cx`.
fn fetch_into(env: &Env<'_>, from: usize, last: usize, cx: &mut Cx<'_, '_>) {
    let mut out = Vec::new();
    fetch(env, from, last, &mut out);
    for command in out {
        cx.command(command);
    }
}

impl Screen for FeedsScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("news.title"),
            None,
            self.list.paging(len),
        );
        let feeds = &self.feeds;
        let count = feeds.len();
        self.list
            .draw(canvas, &style, len, |canvas, row, rect, focused| {
                let (title, value, note) = if row == 0 {
                    let note = if count == 0 {
                        env.i18n.tr("news.empty").to_string()
                    } else {
                        env.i18n
                            .format("news.fetch.help", &[("folder", Arg::Str(FOLDER))])
                    };
                    (env.i18n.tr("news.fetch").to_string(), "", note)
                } else if row == count + 1 {
                    let note = env.i18n.tr("news.add.help").to_string();
                    (env.i18n.tr("news.add").to_string(), "›", note)
                } else {
                    let feed = &feeds[row - 1];
                    (feed.shown(), "", feed.url.clone())
                };
                ui::draw_noted_row(canvas, env, &style, rect, focused, &title, value, &note);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len();
        let rows = self.list.rows() as isize;
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.open(self.list.focus, cx);
            return Handled::Yes;
        } else if *a == actions::MENU {
            self.offer(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        let change = self.list.move_focus(delta, len);
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let len = self.len();
        if gesture.kind == GestureKind::Tap
            && let Some(action) =
                self.header
                    .tap(&env, &self.style, gesture.start, self.list.paging(len))
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(row) = self.list.hit(gesture.start, len) else {
            return Handled::No;
        };
        self.list.focus = row;
        match gesture.kind {
            GestureKind::Tap => self.open(row, cx),
            GestureKind::LongPress => self.offer(row, cx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if *event == ADDED {
            let url = params
                .get(params::TEXT)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) || url.len() < 10 {
                cx.command(Command::Notify(env.i18n.tr("news.bad-address").to_string()));
                return;
            }
            if !self.feeds.iter().any(|f| f.url == url) {
                self.feeds.push(Feed {
                    url,
                    name: String::new(),
                });
                save(env.storage, &self.feeds);
            }
            let index = self.feeds.len() - 1;
            fetch_into(&env, index, index, cx);
            self.redraw(cx);
        } else if *event == CHOSEN {
            let Some(index) = params
                .get(params::PATH)
                .and_then(|n| n.parse::<usize>().ok())
            else {
                return;
            };
            match params.get(choice::OPTION).unwrap_or_default() {
                "open" => self.open(index + 1, cx),
                "fetch" => fetch_into(&env, index, index, cx),
                "remove" if index < self.feeds.len() => {
                    let feed = self.feeds.remove(index);
                    save(env.storage, &self.feeds);
                    // Its book goes too.
                    let _ = env.storage.remove(&feed.book(&env));
                    self.list.focus = self.list.focus.min(self.len() - 1);
                    self.redraw(cx);
                }
                _ => {}
            }
        } else if *event == FETCHED {
            // Names may have come with the feeds.
            self.feeds = load(env.storage);
            self.redraw(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_i18n::{Catalog, check};
    use quireink_sdk::MemoryStorage;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn short_titles() {
        assert_eq!(
            short_title("tagesschau.de - die erste Adresse für Nachrichten"),
            "tagesschau.de"
        );
        assert_eq!(short_title("Blog | Thoughts"), "Blog");
        assert_eq!(short_title("A - b"), "A - b");
        assert_eq!(short_title("Plain"), "Plain");
    }

    #[test]
    fn feeds_are_kept() {
        let storage = MemoryStorage::new();
        let feeds = [
            Feed {
                url: String::from("https://example.org/feed.xml"),
                name: String::from("Example\tnews"),
            },
            Feed {
                url: String::from("https://www.other.org/rss"),
                name: String::new(),
            },
        ];
        save(&storage, &feeds);
        let loaded = load(&storage);
        assert_eq!(loaded[0].name, "Example news");
        assert_eq!(loaded[1].shown(), "other.org");
    }
}
