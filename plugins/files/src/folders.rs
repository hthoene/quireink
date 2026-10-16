//! Chooses a folder for a setting, e.g. the library folder: browse the
//! device's folders and pick one, without typing a path.
//!
//! The first row uses the shown folder, the next goes to its parent and
//! *New folder* makes one in it; then come its subfolders. Like the
//! library, only one page of folders is held in memory.
//!
//! Reading a folder can take a while (SD cards on Android): the new
//! folder's name and "Opening the folder" show first, the folder is read
//! right after, and taps meanwhile are ignored.
//!
//! Bound to `core.choose-folder`: a setting gets the folder, or the event
//! the caller names comes back with it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode, Screen,
    SettingValue,
};

use quireink_sdk::library::listing::{Entry, Listing};

/// The setting the chosen folder is written to.
pub(crate) const ID: &str = "id";
/// The keyboard's reply with a new folder's name.
const NAMED: Id = Id::new_static("files.folder-named");

/// What a row does.
enum Row<'a> {
    /// Use the shown folder.
    Use,
    /// Go to the parent folder.
    Up,
    /// Make a folder in the shown one.
    New,
    Folder(&'a Entry),
}

/// A folder still to be read.
#[derive(Debug)]
enum Pending {
    /// Show `dir` from its first row.
    Open,
    /// Show `dir`, focused on its subfolder `child` (going up).
    Up(Entry),
}

#[derive(Debug)]
pub(crate) struct FolderChooser {
    /// The setting to change.
    setting: Option<Id>,
    /// Else the event to emit, with the parameters it was opened with.
    reply: Option<(Id, Params)>,
    /// The shown folder; empty for the top of the storage.
    dir: String,
    /// Number of subfolders.
    len: usize,
    /// The subfolders on the visible page, from index `first` on.
    page: Vec<Entry>,
    first: usize,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// The shown folder is still to be read.
    pending: Option<Pending>,
}

impl FolderChooser {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let setting = params.get(ID).and_then(|id| Id::parse(id).ok());
        let reply = params
            .get(params::REPLY)
            .and_then(|r| Id::parse(r).ok())
            .map(|r| (r, params.clone()));
        // Start in the current folder if it still exists.
        let current = match (&setting, params.get(params::TEXT)) {
            (_, Some(text)) => text.trim_matches('/').to_string(),
            (Some(id), None) => env.settings.text(id.as_str()).trim_matches('/').to_string(),
            (None, None) => String::new(),
        };
        let dir = if !current.is_empty() && env.storage.exists(&current) {
            current
        } else {
            String::new()
        };
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            setting,
            reply,
            dir,
            len: 0,
            page: Vec::new(),
            first: 0,
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            // Read once the screen shows.
            pending: Some(Pending::Open),
        }
    }

    /// Shows `self.dir` as being opened and reads it right after.
    fn open_later(&mut self, pending: Pending, cx: &mut Cx<'_, '_>) {
        self.pending = Some(pending);
        self.len = 0;
        self.page.clear();
        self.list.focus = 0;
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
        let now = cx.env().now;
        cx.wake_at(now);
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

    /// Rows before the subfolders: *use*, below the top *up*, *new*.
    fn fixed(&self) -> usize {
        if self.dir.is_empty() { 2 } else { 3 }
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
            1 if self.fixed() == 3 => Some(Row::Up),
            i if i + 1 == self.fixed() => Some(Row::New),
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
                cx.command(Command::Back);
                if let Some(id) = self.setting.clone() {
                    let value = SettingValue::Text(self.dir.clone().into());
                    cx.command(Command::SetSetting(id, value));
                }
                if let Some((reply, params)) = &self.reply {
                    let params = params.clone().with(params::PATH, self.dir.clone());
                    cx.command(Command::Emit(reply.clone(), params));
                }
            }
            Some(Row::New) => {
                cx.command(Command::OpenSlot(
                    slots::KEYBOARD,
                    Params::new()
                        .with(params::TITLE, env.i18n.tr("files.choose.new").to_string())
                        .with(params::REPLY, NAMED.as_str()),
                ));
            }
            Some(Row::Up) => {
                self.up(cx);
            }
            Some(Row::Folder(entry)) => {
                self.dir = if self.dir.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", self.dir, entry.name)
                };
                self.open_later(Pending::Open, cx);
            }
            None => {}
        }
    }

    /// Goes to the parent folder, to focus the folder it came from;
    /// `false` at the top.
    fn up(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.dir.is_empty() {
            return false;
        }
        let child = Entry::folder(self.dir.rsplit('/').next().unwrap_or_default());
        self.dir = self
            .dir
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| parent.to_string());
        self.open_later(Pending::Up(child), cx);
        true
    }

    /// Reads the folder still to be read.
    fn read(&mut self, env: &Env<'_>) {
        match self.pending.take() {
            Some(Pending::Open) => self.load(env),
            Some(Pending::Up(child)) => {
                let (len, rank) = {
                    let listing = self.listing(env);
                    (listing.count(), listing.rank(&child))
                };
                self.len = len;
                self.list.focus = (self.fixed() + rank).min(self.len().saturating_sub(1));
                self.load_page(env);
            }
            None => {}
        }
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
        let title = match style.font(env) {
            // Long paths keep their end, which tells where one is.
            Some(font) => {
                let width = f32::from(self.header.area.width) * 0.6;
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
        if self.pending.is_some() {
            self.header.draw(canvas, env, &style, &title, None, None);
            let text = env.i18n.tr("files.choose.opening");
            ui::draw_message(canvas, env, &style, self.list.area, text);
            return;
        }
        self.header
            .draw(canvas, env, &style, &title, None, self.list.paging(len));
        let this = &*self;
        let rows: Vec<(usize, String, &str)> = (0..len)
            .skip(this.list.page() * this.list.rows())
            .take(this.list.rows())
            .filter_map(|index| {
                let row = this.row(index)?;
                Some(match row {
                    Row::Use => (index, env.i18n.tr("files.choose.use").to_string(), ""),
                    Row::Up => (index, format!("‹ {}", env.i18n.tr("files.choose.up")), ""),
                    Row::New => (index, env.i18n.tr("files.choose.new").to_string(), "+"),
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

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if self.pending.is_some() {
            let now = cx.env().now;
            cx.wake_at(now);
        }
    }

    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        if self.pending.is_none() {
            return;
        }
        let env = *cx.env();
        self.read(&env);
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event != NAMED {
            return;
        }
        let env = *cx.env();
        let name = params.get(params::TEXT).unwrap_or_default();
        let name: String = name
            .chars()
            .filter(|c| !matches!(c, '/' | '\\' | ':') && !c.is_control())
            .collect();
        let name = name.trim().trim_start_matches('.');
        if name.is_empty() {
            return;
        }
        let path = quireink_sdk::files::join(&self.dir, name);
        if env.storage.create_dir(&path).is_ok() {
            self.dir = path;
            self.load(&env);
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Partial);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        // While a folder is read, buttons wait (back still leaves).
        if self.pending.is_some() && *action != actions::BACK {
            return Handled::Yes;
        }
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
            if !self.up(cx) {
                return Handled::No;
            }
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        // Taps while a folder is read would land on rows not shown yet.
        if self.pending.is_some() {
            return Handled::Yes;
        }
        let paging = self.list.paging(self.len());
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
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
