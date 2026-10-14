//! Chooses a folder for a setting, e.g. the library folder: browse the
//! device's folders and pick one, without typing a path.
//!
//! The first row uses the shown folder, the second goes to its parent;
//! then come its subfolders. Like the library, only one page of folders is
//! held in memory.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, SettingValue,
};

use crate::listing::{Entry, Listing};

/// The setting the chosen folder is written to.
pub(crate) const ID: &str = "id";

/// What a row does.
enum Row<'a> {
    /// Use the shown folder.
    Use,
    /// Go to the parent folder.
    Up,
    Folder(&'a Entry),
}

#[derive(Debug)]
pub(crate) struct FolderChooser {
    /// The setting to change.
    setting: Option<Id>,
    /// The shown folder; empty for the top of the storage.
    dir: String,
    /// Number of subfolders.
    len: usize,
    /// The subfolders on the visible page, from index `first` on.
    page: Vec<Entry>,
    first: usize,
    style: UiStyle,
    header: Rect,
    list: ListView,
}

impl FolderChooser {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let setting = params.get(ID).and_then(|id| Id::parse(id).ok());
        // Start in the current folder if it still exists.
        let current = setting
            .as_ref()
            .map(|id| env.settings.text(id.as_str()).trim_matches('/').to_string())
            .unwrap_or_default();
        let dir = if !current.is_empty() && env.storage.list(&current, &mut |_| {}).is_ok() {
            current
        } else {
            String::new()
        };
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = Rect::new(0, 0, display.width, style.header as u16);
        let top = header.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let mut chooser = Self {
            setting,
            dir,
            len: 0,
            page: Vec::new(),
            first: 0,
            list: ListView::new(area, style.row),
            style,
            header,
        };
        chooser.load(env);
        chooser
    }

    fn listing<'a>(&'a self, env: &'a Env<'_>) -> Listing<'a> {
        Listing {
            storage: env.storage,
            formats: env.formats,
            dir: &self.dir,
            folders_only: true,
            rank: None,
        }
    }

    /// Rows before the subfolders: *use* and, below the top, *up*.
    fn fixed(&self) -> usize {
        if self.dir.is_empty() { 1 } else { 2 }
    }

    fn len(&self) -> usize {
        self.fixed() + self.len
    }

    /// Shows the folder `self.dir` from its first row.
    fn load(&mut self, env: &Env<'_>) {
        let len = self.listing(env).count();
        self.len = len;
        self.list.focus = 0;
        self.load_page(env);
    }

    /// Loads the subfolders of the page the focus is on.
    fn load_page(&mut self, env: &Env<'_>) {
        let rows = self.list.rows();
        let start = self.list.page() * rows;
        let fixed = self.fixed();
        let from = start.max(fixed);
        let to = (start + rows).min(self.len());
        self.first = from - fixed;
        let page = self.listing(env).range(self.first, to.saturating_sub(from));
        self.page = page;
    }

    fn row(&self, index: usize) -> Option<Row<'_>> {
        match index {
            0 => Some(Row::Use),
            1 if self.fixed() == 2 => Some(Row::Up),
            _ => self
                .page
                .get((index - self.fixed()).checked_sub(self.first)?)
                .map(Row::Folder),
        }
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match self.row(index) {
            Some(Row::Use) => {
                if let Some(id) = self.setting.clone() {
                    let value = SettingValue::Text(self.dir.clone().into());
                    cx.command(Command::SetSetting(id, value));
                }
                cx.command(Command::Back);
            }
            Some(Row::Up) => {
                self.up(&env);
                cx.request_redraw();
            }
            Some(Row::Folder(entry)) => {
                self.dir = if self.dir.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", self.dir, entry.name)
                };
                self.load(&env);
                cx.request_redraw();
            }
            None => {}
        }
    }

    /// Goes to the parent folder, focusing the folder it came from;
    /// `false` at the top.
    fn up(&mut self, env: &Env<'_>) -> bool {
        if self.dir.is_empty() {
            return false;
        }
        let child = Entry::folder(self.dir.rsplit('/').next().unwrap_or_default());
        self.dir = self
            .dir
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| parent.to_string());
        let (len, rank) = {
            let listing = self.listing(env);
            (listing.count(), listing.rank(&child))
        };
        self.len = len;
        self.list.focus = (self.fixed() + rank).min(self.len().saturating_sub(1));
        self.load_page(env);
        true
    }

    fn moved(&mut self, change: ListChange, cx: &mut Cx<'_, '_>) {
        if change == ListChange::Page {
            let env = *cx.env();
            self.load_page(&env);
        }
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
    }
}

impl Screen for FolderChooser {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        let title = if self.dir.is_empty() {
            env.i18n.tr("files.choose.top").to_string()
        } else {
            self.dir.clone()
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
        let title = match style.font(env) {
            // Long paths keep their end, which tells where one is.
            Some(font) => {
                let width = f32::from(self.header.width) * 0.75;
                if font.measure(style.title, &title) > width {
                    let tail: String = title.chars().rev().collect();
                    let fitted = ui::fit(&font, style.title, &tail, width);
                    fitted.chars().rev().collect()
                } else {
                    title
                }
            }
            None => title,
        };
        ui::draw_header(canvas, env, &style, self.header, &title, note.as_deref());
        let this = &*self;
        let rows: Vec<(usize, String, &str)> = (0..len)
            .skip(this.list.page() * this.list.rows())
            .take(this.list.rows())
            .filter_map(|index| {
                let row = this.row(index)?;
                Some(match row {
                    Row::Use => (index, env.i18n.tr("files.choose.use").to_string(), ""),
                    Row::Up => (index, format!("‹ {}", env.i18n.tr("files.choose.up")), ""),
                    Row::Folder(entry) => (index, format!("{}/", entry.name), "›"),
                })
            })
            .collect();
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                if let Some((_, title, value)) = rows.iter().find(|(i, _, _)| *i == index) {
                    ui::draw_value_row(canvas, env, &style, rect, focused, title, value);
                }
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len();
        let rows = self.list.rows() as isize;
        let a = action;
        let delta = if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            Some(-1)
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            Some(1)
        } else if *a == actions::NEXT_PAGE {
            Some(rows)
        } else if *a == actions::PREV_PAGE {
            Some(-rows)
        } else {
            None
        };
        if let Some(delta) = delta {
            let change = self.list.move_focus(delta, len);
            self.moved(change, cx);
        } else if *a == actions::CONFIRM {
            self.activate(self.list.focus, cx);
        } else if *a == actions::BACK {
            // Back goes up first; at the top it closes the chooser.
            let env = *cx.env();
            if !self.up(&env) {
                return Handled::No;
            }
            cx.request_redraw();
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        match self.list.hit(gesture.start, self.len()) {
            Some(index) => {
                self.list.focus = index;
                self.activate(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}
