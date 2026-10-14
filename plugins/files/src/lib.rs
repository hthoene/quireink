//! A simple file browser for the library folder.
//!
//! It lists folders and every file a format plugin can open, and opens books
//! in the `core.reader` slot. It is bound to the `core.home` and
//! `core.library` slots with default priority, so a dashboard or bookshelf
//! plugin can take over the home screen while this stays the library view.
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

use quireink_sdk::builtin::{actions, fonts, params, slots};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params,
    Plugin, Priority, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
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
        r.bind_slot(slots::HOME, SCREEN, Priority::DEFAULT)?;
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

/// Row geometry, derived from the screen and the UI font.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    header: u32,
    /// Space between the header and the first row.
    gap: u32,
    row: u32,
    rows: usize,
    margin: u32,
    size: f32,
}

#[derive(Debug)]
struct Browser {
    root: String,
    dir: String,
    entries: Vec<Entry>,
    focus: usize,
    geometry: Geometry,
}

impl Browser {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let root = env
            .settings
            .text(ROOT.as_str())
            .trim_matches('/')
            .to_string();
        let dir = params.get(params::PATH).unwrap_or(&root).to_string();
        let size = env.metrics.pt(11.0);
        let line = env
            .fonts
            .role(fonts::UI.as_str())
            .map_or(size * 1.2, |f| f.line_metrics(size).line_height);
        let header = env.metrics.mm(10);
        let footer = env.metrics.mm(7);
        let row = (line * 1.8) as u32;
        let gap = env.metrics.mm(2);
        let available = u32::from(env.device.display.height).saturating_sub(header + gap + footer);
        let mut browser = Self {
            root,
            dir,
            entries: Vec::new(),
            focus: 0,
            geometry: Geometry {
                header,
                row: row.max(1),
                rows: (available / row.max(1)).max(1) as usize,
                gap,
                margin: env.metrics.mm(3),
                size,
            },
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
        self.focus = 0;
    }

    fn path_of(&self, entry: &Entry) -> String {
        if self.dir.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", self.dir, entry.name)
        }
    }

    fn page(&self) -> usize {
        self.focus / self.geometry.rows
    }

    fn pages(&self) -> usize {
        self.entries.len().div_ceil(self.geometry.rows).max(1)
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let focus = (self.focus as isize + delta).clamp(0, last) as usize;
        if focus != self.focus {
            let page_changed = focus / self.geometry.rows != self.page();
            self.focus = focus;
            cx.request_redraw();
            cx.request_refresh(if page_changed {
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
        self.focus = self
            .entries
            .iter()
            .position(|e| e.is_dir && e.name == child)
            .unwrap_or(0);
        cx.request_redraw();
        true
    }

    fn row_top(&self, row: usize) -> i32 {
        (self.geometry.header + self.geometry.gap + row as u32 * self.geometry.row) as i32
    }
}

impl Screen for Browser {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let Some(font) = env.fonts.role(fonts::UI.as_str()) else {
            return;
        };
        let g = self.geometry;
        let width = u32::from(env.device.display.width);
        let height = u32::from(env.device.display.height);
        let size = g.size;
        let metrics = font.line_metrics(size);
        let centred =
            |top: i32, h: u32| top + ((h as f32 + metrics.ascent - metrics.descent) / 2.0) as i32;

        // Header: library title or the current folder.
        canvas.fill_rect(0, 0, width, g.header, Color::BLACK);
        let title = if self.dir == self.root {
            env.i18n.tr("files.title").to_string()
        } else {
            self.dir.rsplit('/').next().unwrap_or_default().to_string()
        };
        font.draw(
            canvas,
            size * 1.15,
            g.margin as f32,
            centred(0, g.header),
            &title,
            Color::WHITE,
        );

        if self.entries.is_empty() {
            let message = env
                .i18n
                .format("files.empty", &[("dir", Arg::Str(&self.dir))]);
            let hint = env.i18n.tr("files.hint");
            let max = (width - 2 * g.margin) as f32;
            let mut baseline = (height / 3) as i32;
            for text in [message.as_str(), hint] {
                for range in font.wrap(size, text, max) {
                    font.draw(
                        canvas,
                        size,
                        g.margin as f32,
                        baseline,
                        &text[range],
                        Color::BLACK,
                    );
                    baseline += metrics.line_height as i32;
                }
                baseline += (metrics.line_height / 2.0) as i32;
            }
            return;
        }

        let first = self.page() * g.rows;
        for (row, entry) in self.entries.iter().enumerate().skip(first).take(g.rows) {
            let slot = row - first;
            let top = self.row_top(slot);
            let focused = row == self.focus;
            let (background, foreground) = if focused {
                (Color::BLACK, Color::WHITE)
            } else {
                (Color::WHITE, Color::BLACK)
            };
            if focused {
                canvas.fill_rect(0, top, width, g.row, background);
            } else {
                canvas.fill_rect(
                    g.margin as i32,
                    top + g.row as i32 - 1,
                    width - 2 * g.margin,
                    1,
                    Color::gray(180),
                );
            }
            let name = if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry
                    .name
                    .rsplit_once('.')
                    .map_or(entry.name.clone(), |(stem, _)| stem.to_string())
            };
            font.draw(
                canvas,
                size,
                g.margin as f32,
                centred(top, g.row),
                &name,
                foreground,
            );
        }

        if self.pages() > 1 {
            let label = env.i18n.format(
                "files.page",
                &[
                    ("page", Arg::Int(self.page() as i64 + 1)),
                    ("pages", Arg::Int(self.pages() as i64)),
                ],
            );
            let small = env.metrics.pt(8.0);
            let x = width as f32 - g.margin as f32 - font.measure(small, &label);
            let baseline = height as i32 - env.metrics.mm(2) as i32;
            font.draw(canvas, small, x, baseline, &label, Color::gray(80));
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let rows = self.geometry.rows as isize;
        let a = action;
        if *a == actions::UP || *a == actions::LEFT {
            self.move_focus(-1, cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT {
            self.move_focus(1, cx);
        } else if *a == actions::NEXT_PAGE {
            self.move_focus(rows, cx);
        } else if *a == actions::PREV_PAGE {
            self.move_focus(-rows, cx);
        } else if *a == actions::CONFIRM {
            self.open(self.focus, cx);
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
        let y = gesture.start.y - (self.geometry.header + self.geometry.gap) as i32;
        if y < 0 {
            return Handled::No;
        }
        let slot = (y as u32 / self.geometry.row) as usize;
        if slot >= self.geometry.rows {
            return Handled::No;
        }
        let index = self.page() * self.geometry.rows + slot;
        if index >= self.entries.len() {
            return Handled::No;
        }
        self.focus = index;
        self.open(index, cx);
        Handled::Yes
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
