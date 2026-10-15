//! A file manager: every folder and file of the device's storage, to
//! open, rename, move, copy and delete, and to make folders.
//!
//! - Tapping a folder opens it, *Back* goes to the folder above.
//! - Tapping a file, or holding any entry (with buttons: a long press on
//!   confirm), offers what can be done with it; deleting asks first.
//! - *Move* and *Copy* pick the entry: then go to the folder it belongs in
//!   and choose *Paste here* in the bar at the bottom.
//! - Files starting with `.` (e.g. QuireInk's own data) are hidden unless
//!   the setting *Show hidden files* says otherwise.
//!
//! With buttons only, down from the last entry reaches the bar.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, groups, menus, params, slots};
use quireink_sdk::ui::choice::{self, Choice};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

use quireink_sdk::files::{self as ops, Entry};

pub const PLUGIN: Id = Id::new_static("file-manager");
/// Opens the file manager.
pub const OPEN: Id = Id::new_static("file-manager.open");
/// Shows files and folders starting with `.`.
pub const HIDDEN: Id = Id::new_static("file-manager.hidden");
const SCREEN: Id = Id::new_static("file-manager.browser");
const CHOICE: Id = Id::new_static("file-manager.choice");
/// Replies of the choice overlay and the keyboard.
const CHOSEN: Id = Id::new_static("file-manager.chosen");
const RENAMED: Id = Id::new_static("file-manager.renamed");
const FOLDER_NAMED: Id = Id::new_static("file-manager.folder-named");
/// Translation keys of the choices: this, then the option.
const OPTION_LABELS: &str = "file-manager.option.";

/// What was picked to be moved or copied.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Picked {
    path: String,
    copy: bool,
}

/// The buttons of the bar at the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    NewFolder,
    Paste,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Bar(usize),
}

/// The file manager plugin.
#[derive(Debug, Default)]
pub struct FileManager;

impl Plugin for FileManager {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("File manager"),
            description: Cow::Borrowed(
                "Every folder and file on the device: open, rename, move, copy and delete them.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_setting(
            SettingSpec::new(
                HIDDEN,
                "file-manager.setting.hidden",
                SettingKind::Bool,
                SettingValue::Bool(false),
            )
            .group(groups::SYSTEM)
            .advanced()
            .description("file-manager.setting.hidden.help"),
        )?;
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("file-manager.title"),
            factory: Box::new(|env, params| Box::new(Browser::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("file-manager.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("file-manager.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::MAIN,
            item: OPEN,
            order: 45,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("folder"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

struct Browser {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    bar: Rect,
    /// The shown folder; empty at the top of the storage.
    dir: String,
    entries: Vec<Entry>,
    picked: Option<Picked>,
    focus: Focus,
    /// What happened last (an error, or what was done).
    message: Option<String>,
}

impl Browser {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let bar_height = style.row as u16;
        let top = header.area.height + style.gap as u16;
        // The bar, and a line for messages above it.
        let bottom = bar_height * 2 + style.gap as u16;
        let area = Rect::new(
            0,
            top,
            display.width,
            display.height.saturating_sub(top + bottom),
        );
        let mut browser = Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            bar: Rect::new(0, display.height - bar_height, display.width, bar_height),
            dir: params.get(params::PATH).unwrap_or_default().to_string(),
            entries: Vec::new(),
            picked: None,
            focus: Focus::List,
            message: None,
            style,
            header,
        };
        browser.load(env);
        browser
    }

    fn load(&mut self, env: &Env<'_>) {
        let hidden = env.settings.bool(HIDDEN.as_str());
        match ops::list(env.storage, &self.dir, hidden) {
            Ok(entries) => self.entries = entries,
            Err(e) => {
                self.entries.clear();
                self.message = Some(error(env, e));
            }
        }
        self.list.focus = self.list.focus.min(self.entries.len().saturating_sub(1));
    }

    fn buttons(&self) -> Vec<Button> {
        match &self.picked {
            Some(_) => Vec::from([Button::Paste, Button::Cancel]),
            None => Vec::from([Button::NewFolder]),
        }
    }

    fn button_rect(&self, index: usize) -> Rect {
        let count = self.buttons().len() as u16;
        let margin = self.style.margin as u16;
        let gap = self.style.gap as u16 * 2;
        let width = (self.bar.width - 2 * margin - gap * (count - 1)) / count;
        let height = ui::button_height(self.bar);
        Rect::new(
            margin + index as u16 * (width + gap),
            self.bar.y + (self.bar.height - height) / 2,
            width,
            height,
        )
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    /// Opens folder `dir` from its first entry.
    fn enter(&mut self, dir: String, cx: &mut Cx<'_, '_>) {
        self.dir = dir;
        self.list.focus = 0;
        self.focus = Focus::List;
        let env = *cx.env();
        self.load(&env);
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    /// Back to the folder above; `false` at the top.
    fn up(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.dir.is_empty() {
            return false;
        }
        let name = ops::name(&self.dir).to_string();
        let parent = ops::parent(&self.dir).to_string();
        self.enter(parent, cx);
        // The folder just left stays chosen.
        if let Some(at) = self.entries.iter().position(|e| e.name == name) {
            self.list.focus = at;
        }
        true
    }

    /// Tapped or confirmed: a folder opens, a file offers its actions.
    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some(entry) = self.entries.get(index).cloned() else {
            return;
        };
        let path = ops::join(&self.dir, &entry.name);
        if entry.is_dir {
            self.enter(path, cx);
        } else {
            self.offer(index, cx);
        }
    }

    /// The actions for entry `index`.
    fn offer(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        let path = ops::join(&self.dir, &entry.name);
        let env = *cx.env();
        let mut options = Vec::new();
        if !entry.is_dir && env.formats.supports(&path) {
            options.push("open");
        }
        options.extend(["rename", "move", "copy", "delete"]);
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &entry.name,
            &path,
            OPTION_LABELS,
            &options,
        ));
    }

    fn press(&mut self, button: Button, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match button {
            Button::NewFolder => cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                Params::new()
                    .with(
                        params::TITLE,
                        env.i18n.tr("file-manager.new-folder").to_string(),
                    )
                    .with(params::REPLY, FOLDER_NAMED.as_str()),
            )),
            Button::Cancel => {
                self.picked = None;
                self.message = None;
                self.focus = Focus::List;
                self.redraw(cx);
            }
            Button::Paste => {
                let Some(picked) = self.picked.take() else {
                    return;
                };
                let target = ops::free_path(env.storage, &self.dir, ops::name(&picked.path));
                let result = if picked.copy {
                    ops::copy(env.storage, &picked.path, &target)
                } else if ops::parent(&picked.path) == self.dir {
                    // Moved where it is: nothing to do.
                    Ok(())
                } else {
                    let moved = ops::move_to(env.storage, &picked.path, &target);
                    if moved.is_ok() {
                        tell_moved(&env, &picked.path, &target, cx);
                    }
                    moved
                };
                self.message = result.err().map(|e| error(&env, e));
                self.focus = Focus::List;
                self.load(&env);
                if let Some(at) = self
                    .entries
                    .iter()
                    .position(|e| e.name == ops::name(&target))
                {
                    self.list.focus = at;
                }
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
        }
    }

    /// The choice made in the overlay for `path`.
    fn chosen(&mut self, option: &str, path: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let name = ops::name(path).to_string();
        match option {
            "open" => cx.command(Command::OpenSlot(
                slots::READER,
                Params::new().with(params::PATH, path.to_string()),
            )),
            "rename" => cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                Params::new()
                    .with(
                        params::TITLE,
                        env.i18n.tr("file-manager.rename").to_string(),
                    )
                    .with(params::TEXT, name)
                    .with(params::PATH, path.to_string())
                    .with(params::REPLY, RENAMED.as_str()),
            )),
            "move" | "copy" => {
                let copy = option == "copy";
                self.picked = Some(Picked {
                    path: path.to_string(),
                    copy,
                });
                let key = if copy {
                    "file-manager.picked-copy"
                } else {
                    "file-manager.picked-move"
                };
                self.message = Some(env.i18n.format(key, &[("name", Arg::Str(&name))]));
                self.redraw(cx);
            }
            "delete" => {
                let title = env
                    .i18n
                    .format("file-manager.delete-question", &[("name", Arg::Str(&name))]);
                cx.command(choice::open(
                    CHOICE,
                    CHOSEN,
                    &title,
                    path,
                    OPTION_LABELS,
                    &["delete-yes", "cancel"],
                ));
            }
            "delete-yes" => {
                self.message = ops::delete(env.storage, path).err().map(|e| error(&env, e));
                if self.picked.as_ref().is_some_and(|p| p.path == path) {
                    self.picked = None;
                }
                self.load(&env);
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
            _ => {}
        }
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        let len = self.entries.len();
        let bar = self.buttons().len();
        self.focus = match self.focus {
            Focus::List if delta > 0 && (len == 0 || self.list.focus + 1 >= len) => Focus::Bar(0),
            Focus::List => {
                let change = self.list.move_focus(delta, len);
                if change == ListChange::Page {
                    cx.request_refresh(RefreshMode::Partial);
                }
                Focus::List
            }
            Focus::Bar(i) if delta > 0 => Focus::Bar((i + 1).min(bar - 1)),
            Focus::Bar(0) => Focus::List,
            Focus::Bar(i) => Focus::Bar(i - 1),
        };
        self.redraw(cx);
    }
}

/// What to tell the user about a failed operation.
fn error(env: &Env<'_>, e: quireink_sdk::StorageError) -> String {
    use quireink_sdk::StorageError as E;
    let key = match e {
        E::NotFound => "file-manager.error.not-found",
        E::InvalidPath => "file-manager.error.invalid",
        E::Unsupported => "file-manager.error.unsupported",
        E::Full => "file-manager.error.full",
        _ => "file-manager.error.io",
    };
    env.i18n.tr(key).to_string()
}

/// A name for a file or folder: one line, no `/`, not `.` or `..`.
fn valid_name(name: &str) -> Option<&str> {
    let name = name.trim();
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':', '\0'])
        || name.chars().any(char::is_control);
    (!bad).then_some(name)
}

impl Screen for Browser {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.entries.len();
        let title = if self.dir.is_empty() {
            env.i18n.tr("file-manager.device").to_string()
        } else {
            ops::name(&self.dir).to_string()
        };
        self.header
            .draw(canvas, env, &style, &title, None, self.list.paging(len));
        if len == 0 {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("file-manager.empty"),
            );
        }
        let decimal = env.i18n.tr("file-manager.decimal");
        let entries = &self.entries;
        let active = self.focus == Focus::List;
        self.list.active = active;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let entry = &entries[index];
                let value = if entry.is_dir {
                    String::from("›")
                } else {
                    ops::size(entry.size, decimal)
                };
                let name = if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                ui::draw_value_row(canvas, env, &style, rect, focused, &name, &value);
            });
        // The message line above the bar.
        if let (Some(message), Some(font)) = (&self.message, style.font(env)) {
            let line = Rect::new(
                0,
                self.bar.y - style.row as u16,
                self.bar.width,
                style.row as u16,
            );
            ui::draw_label(
                canvas,
                &font,
                style.small,
                style.margin as f32,
                line,
                message,
                Color::gray(60),
            );
        }
        for (i, button) in self.buttons().into_iter().enumerate() {
            let label = env.i18n.tr(match button {
                Button::NewFolder => "file-manager.new-folder",
                Button::Paste => "file-manager.paste",
                Button::Cancel => "file-manager.cancel",
            });
            let focused = self.focus == Focus::Bar(i);
            let on = button == Button::Paste;
            ui::draw_button(canvas, env, &style, self.button_rect(i), label, on, focused);
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
            let delta = if *a == actions::NEXT_PAGE {
                rows
            } else {
                -rows
            };
            if self.list.move_focus(delta, self.entries.len()) != ListChange::None {
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
        } else if *a == actions::CONFIRM {
            match self.focus {
                Focus::List => self.activate(self.list.focus, cx),
                Focus::Bar(i) => {
                    if let Some(button) = self.buttons().get(i).copied() {
                        self.press(button, cx);
                    }
                }
            }
        } else if *a == actions::MENU && self.focus == Focus::List {
            self.offer(self.list.focus, cx);
        } else if *a == actions::BACK {
            if !self.up(cx) {
                cx.command(Command::Back);
            }
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let paging = self.list.paging(self.entries.len());
        if gesture.kind == GestureKind::Tap
            && let Some(action) = self
                .header
                .tap(cx.env(), &self.style, gesture.start, paging)
        {
            // Back goes up a folder first.
            if action == actions::BACK {
                if !self.up(cx) {
                    cx.command(Command::Back);
                }
            } else {
                cx.command(Command::RunAction(action));
            }
            return Handled::Yes;
        }
        if let Some(i) =
            (0..self.buttons().len()).find(|&i| self.button_rect(i).contains(gesture.start))
        {
            if gesture.kind == GestureKind::Tap
                && let Some(button) = self.buttons().get(i).copied()
            {
                self.press(button, cx);
            }
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, self.entries.len()) else {
            return Handled::No;
        };
        self.list.focus = index;
        self.focus = Focus::List;
        cx.request_redraw_area(self.list.area);
        match gesture.kind {
            GestureKind::Tap => self.activate(index, cx),
            GestureKind::LongPress => self.offer(index, cx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let text = params.get(params::TEXT).unwrap_or_default();
        let path = params.get(params::PATH).unwrap_or_default().to_string();
        if *event == CHOSEN {
            let option = params.get(choice::OPTION).unwrap_or_default().to_string();
            self.chosen(&option, &path, cx);
        } else if *event == RENAMED {
            let Some(name) = valid_name(text) else {
                self.message = Some(env.i18n.tr("file-manager.error.invalid").to_string());
                return self.redraw(cx);
            };
            if name == ops::name(&path) {
                return;
            }
            let target = ops::free_path(env.storage, ops::parent(&path), name);
            let moved = ops::move_to(env.storage, &path, &target);
            if moved.is_ok() {
                tell_moved(&env, &path, &target, cx);
            }
            self.message = moved.err().map(|e| error(&env, e));
            self.load(&env);
            if let Some(at) = self
                .entries
                .iter()
                .position(|e| e.name == ops::name(&target))
            {
                self.list.focus = at;
            }
            self.redraw(cx);
        } else if *event == FOLDER_NAMED {
            let Some(name) = valid_name(text) else {
                if !text.trim().is_empty() {
                    self.message = Some(env.i18n.tr("file-manager.error.invalid").to_string());
                    self.redraw(cx);
                }
                return;
            };
            let target = ops::free_path(env.storage, &self.dir, name);
            self.message = env
                .storage
                .create_dir(&target)
                .err()
                .map(|e| error(&env, e));
            self.load(&env);
            if let Some(at) = self
                .entries
                .iter()
                .position(|e| e.name == ops::name(&target))
            {
                self.list.focus = at;
            }
            self.redraw(cx);
        }
    }
}

/// Most books whose moving is told for one moved folder.
const MAX_MOVED: usize = 2000;

/// Tells the other plugins that the files at `from` are now at `to`
/// ([`events::BOOK_MOVED`]), so the data they keep by a book's path
/// (progress, notes, statistics) goes along; for a folder, of every file
/// in it.
fn tell_moved(env: &Env<'_>, from: &str, to: &str, cx: &mut Cx<'_, '_>) {
    let mut pending = alloc::vec![(from.to_string(), to.to_string())];
    let mut told = 0;
    while let Some((from, to)) = pending.pop() {
        if !ops::is_dir(env.storage, &to) {
            cx.command(Command::Emit(
                events::BOOK_MOVED,
                Params::new()
                    .with(params::FROM, from)
                    .with(params::PATH, to),
            ));
            told += 1;
            continue;
        }
        for entry in ops::list(env.storage, &to, true).unwrap_or_default() {
            if told + pending.len() >= MAX_MOVED {
                return;
            }
            pending.push((ops::join(&from, &entry.name), ops::join(&to, &entry.name)));
        }
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
    fn names_are_checked() {
        assert_eq!(valid_name("  Neu  "), Some("Neu"));
        assert_eq!(valid_name("a/b"), None);
        assert_eq!(valid_name(".."), None);
        assert_eq!(valid_name(""), None);
    }
}
