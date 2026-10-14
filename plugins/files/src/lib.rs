//! A simple file browser for the library folder.
//!
//! It lists folders and every file a format plugin can open, and opens books
//! in the `core.reader` slot. It is bound to `core.library`, and to
//! `core.home` only as a fallback when no home screen plugin is installed.
//!
//! Works with buttons only (up/down or left/right move the selection,
//! page buttons scroll, confirm opens, back goes to the parent folder) and
//! with touch (tap a row).

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params,
    Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("files");
pub const SCREEN: Id = Id::new_static("files.browser");
/// The library folder, relative to the storage root.
pub const ROOT: Id = Id::new_static("files.root");

/// The file browser plugin.
#[derive(Debug, Default)]
pub struct Files;

impl Plugin for Files {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("File browser"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("files.title"),
            factory: Box::new(|env, params| Box::new(Browser::new(env, params))),
        })?;
        // The library; also the start screen when no home screen plugin
        // is installed.
        r.bind_slot(slots::HOME, SCREEN, Priority::FALLBACK)?;
        r.bind_slot(slots::LIBRARY, SCREEN, Priority::DEFAULT)?;
        r.add_setting(SettingSpec {
            id: ROOT,
            title: Cow::Borrowed("files.setting.root"),
            kind: SettingKind::Text,
            default: SettingValue::Text(Cow::Borrowed("books")),
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    is_dir: bool,
}

#[derive(Debug)]
struct Browser {
    root: String,
    dir: String,
    entries: Vec<Entry>,
    style: UiStyle,
    header: Rect,
    list: ListView,
}

impl Browser {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let root = env
            .settings
            .text(ROOT.as_str())
            .trim_matches('/')
            .to_string();
        let dir = params.get(params::PATH).unwrap_or(&root).to_string();
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = Rect::new(0, 0, display.width, style.header as u16);
        let top = header.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let mut browser = Self {
            root,
            dir,
            entries: Vec::new(),
            list: ListView::new(area, style.row),
            style,
            header,
        };
        browser.load(env);
        browser
    }

    fn load(&mut self, env: &Env<'_>) {
        let mut entries = Vec::new();
        let _ = env.storage.list(&self.dir, &mut |entry| {
            if entry.name.starts_with('.') {
                return;
            }
            if entry.is_dir || env.formats.supports(entry.name) {
                entries.push(Entry {
                    name: entry.name.to_string(),
                    is_dir: entry.is_dir,
                });
            }
        });
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries = entries;
        self.list.focus = 0;
    }

    fn path_of(&self, entry: &Entry) -> String {
        if self.dir.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", self.dir, entry.name)
        }
    }

    fn moved(change: ListChange, cx: &mut Cx<'_, '_>) {
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
    }

    fn open(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some(entry) = self.entries.get(index).cloned() else {
            return;
        };
        let path = self.path_of(&entry);
        if entry.is_dir {
            self.dir = path;
            let env = *cx.env();
            self.load(&env);
            cx.request_redraw();
        } else {
            cx.command(Command::OpenSlot(
                slots::READER,
                Params::new().with(params::PATH, path),
            ));
        }
    }

    /// Goes to the parent folder; `false` at the library root.
    fn up(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.dir == self.root || self.dir.is_empty() {
            return false;
        }
        let child = self.dir.rsplit('/').next().unwrap_or_default().to_string();
        self.dir = self
            .dir
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| parent.to_string());
        let env = *cx.env();
        self.load(&env);
        self.list.focus = self
            .entries
            .iter()
            .position(|e| e.is_dir && e.name == child)
            .unwrap_or(0);
        cx.request_redraw();
        true
    }
}

impl Screen for Browser {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.entries.len();
        let title = if self.dir == self.root {
            env.i18n.tr("files.title").to_string()
        } else {
            self.dir.rsplit('/').next().unwrap_or_default().to_string()
        };
        let note = (self.list.pages(len) > 1).then(|| {
            env.i18n.format(
                "files.page",
                &[
                    ("page", Arg::Int(self.list.page() as i64 + 1)),
                    ("pages", Arg::Int(self.list.pages(len) as i64)),
                ],
            )
        });
        ui::draw_header(canvas, env, &style, self.header, &title, note.as_deref());

        if self.entries.is_empty() {
            let message = format!(
                "{}\n{}",
                env.i18n
                    .format("files.empty", &[("dir", Arg::Str(&self.dir))]),
                env.i18n.tr("files.hint")
            );
            ui::draw_message(canvas, env, &style, self.list.area, &message);
            return;
        }
        let Some(font) = style.font(env) else {
            return;
        };
        let entries = &self.entries;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let entry = &entries[index];
                let color = if focused { Color::WHITE } else { Color::BLACK };
                let name = if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry
                        .name
                        .rsplit_once('.')
                        .map_or(entry.name.clone(), |(stem, _)| stem.to_string())
                };
                let mut width = f32::from(rect.width) - 2.0 * style.margin as f32;
                // The format, small and right-aligned, tells same-named
                // books apart (e.g. TXT and EPUB editions).
                if !entry.is_dir
                    && let Some((_, extension)) = entry.name.rsplit_once('.')
                {
                    let tag = extension.to_uppercase();
                    let tag_width = font.measure(style.small, &tag);
                    let x = f32::from(rect.width) - style.margin as f32 - tag_width;
                    ui::draw_label(canvas, &font, style.small, x, rect, &tag, color);
                    width -= tag_width + style.margin as f32;
                }
                let name = ui::fit(&font, style.text, &name, width);
                ui::draw_label(
                    canvas,
                    &font,
                    style.text,
                    style.margin as f32,
                    rect,
                    &name,
                    color,
                );
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.entries.len();
        let rows = self.list.rows() as isize;
        let a = action;
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            Self::moved(self.list.move_focus(-1, len), cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            Self::moved(self.list.move_focus(1, len), cx);
        } else if *a == actions::NEXT_PAGE {
            Self::moved(self.list.move_focus(rows, len), cx);
        } else if *a == actions::PREV_PAGE {
            Self::moved(self.list.move_focus(-rows, len), cx);
        } else if *a == actions::CONFIRM {
            self.open(self.list.focus, cx);
        } else if *a == actions::BACK {
            return if self.up(cx) {
                Handled::Yes
            } else {
                Handled::No
            };
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        match self.list.hit(gesture.start, self.entries.len()) {
            Some(index) => {
                self.list.focus = index;
                self.open(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED {
            // The UI size or the library folder may have changed.
            let env = *cx.env();
            let dir = self.dir.clone();
            let focus = self.list.focus;
            *self = Self::new(&env, &Params::new().with(params::PATH, dir));
            self.list.focus = focus.min(self.entries.len().saturating_sub(1));
            cx.request_redraw();
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
