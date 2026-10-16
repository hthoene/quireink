//! Customizing menus on the device: reorder, remove and add entries of any
//! menu, or return it to how its plugins define it. Changes apply at once
//! and are saved in `[menus]` of `quireink.ini`.
//!
//! One cannot lock oneself out: a menu that leads to the settings (directly
//! or through submenus) keeps doing so, and this editor stays on the
//! settings page. Back arrows and buttons do not depend on menus anyway.
//!
//! Reached from the main settings page (menu `core.settings-pages`).

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus};
use quireink_sdk::ui::{self, ListChange, ListView, Reorder, ReorderStep, UiStyle};
use quireink_sdk::{
    ActionSpec, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, PointerEvent, Rect, RefreshMode, RegisterError, Registrar,
    SDK_VERSION, Screen, ScreenSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("menu-editor");
/// Opens the editor.
pub const OPEN: Id = Id::new_static("menu-editor.open");
const MENUS_SCREEN: Id = Id::new_static("menu-editor.menus");
const ENTRIES_SCREEN: Id = Id::new_static("menu-editor.entries");
const ENTRY_SCREEN: Id = Id::new_static("menu-editor.entry");
const ADD_SCREEN: Id = Id::new_static("menu-editor.add");
/// An entry moved; parameter `index`: where it is now.
const MOVED: Id = Id::new_static("menu-editor.moved");

/// Parameters: the menu being edited and an entry's index in it.
const MENU: &str = "menu";
const INDEX: &str = "index";

/// What must stay reachable from every menu that reaches it by default.
const PROTECTED: [Id; 2] = [actions::SETTINGS, OPEN];

/// Creates one of the editor's screens.
type OpenScreen = fn(&Env<'_>, &Params) -> Box<dyn Screen>;

/// The menu editor plugin.
#[derive(Debug, Default)]
pub struct MenuEditor;

impl Plugin for MenuEditor {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Menu editor"),
            description: Cow::Borrowed("Reorders, removes and adds the entries of every menu."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let screens: [(Id, OpenScreen); 4] = [
            (MENUS_SCREEN, |env, _| Box::new(MenusScreen::open(env))),
            (ENTRIES_SCREEN, |env, p| {
                Box::new(EntriesScreen::open(env, p))
            }),
            (ENTRY_SCREEN, |env, p| Box::new(EntryScreen::open(env, p))),
            (ADD_SCREEN, |env, p| Box::new(AddScreen::open(env, p))),
        ];
        for (id, open) in screens {
            r.add_screen(ScreenSpec {
                id,
                title: Cow::Borrowed("menu-editor.title"),
                factory: Box::new(open),
            })?;
        }
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("menu-editor.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(MENUS_SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::SETTINGS,
            item: OPEN,
            order: 210,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("menu-2"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

// ---- Menus and their entries -------------------------------------------

/// Whether `id` is a menu.
fn is_menu(env: &Env<'_>, id: &str) -> bool {
    env.menus.menu_ids().iter().any(|m| m.as_str() == id)
}

/// The display name of an entry: an action, a setting or a submenu.
fn name(env: &Env<'_>, id: &str) -> String {
    let key = env
        .actions
        .get(id)
        .map(|a| a.title.as_ref())
        .or_else(|| {
            env.settings
                .specs()
                .find(|s| s.id.as_str() == id)
                .map(|s| s.title.as_ref())
        })
        .unwrap_or(id);
    env.i18n.tr(key).to_string()
}

/// The entries of `menu` as shown now.
fn entries(env: &Env<'_>, menu: &str) -> Vec<Id> {
    env.menus.ids(menu).into_iter().cloned().collect()
}

/// Whether `from` leads to `target`, directly or through submenus, with
/// `ids` giving each menu's entries.
fn reaches(ids: &dyn Fn(&str) -> Vec<Id>, from: &str, target: &Id, seen: &mut Vec<String>) -> bool {
    if seen.iter().any(|s| s == from) {
        return false;
    }
    seen.push(from.to_string());
    ids(from)
        .iter()
        .any(|id| id == target || reaches(ids, id.as_str(), target, seen))
}

/// Whether redefining `menu` as `proposed` keeps every protected entry
/// reachable from every menu that reaches it as its plugins define it.
fn keeps_access(env: &Env<'_>, menu: &str, proposed: &[Id]) -> bool {
    let all = env.menus.menu_ids();
    let is_menu = |id: &str| all.iter().any(|m| m.as_str() == id);
    let defaults = |m: &str| -> Vec<Id> {
        if is_menu(m) {
            env.menus.default_ids(m).into_iter().cloned().collect()
        } else {
            Vec::new()
        }
    };
    let after = |m: &str| -> Vec<Id> {
        if m == menu {
            proposed.to_vec()
        } else if is_menu(m) {
            entries(env, m)
        } else {
            Vec::new()
        }
    };
    all.iter().all(|m| {
        PROTECTED.iter().all(|target| {
            !reaches(&defaults, m.as_str(), target, &mut Vec::new())
                || reaches(&after, m.as_str(), target, &mut Vec::new())
        })
    })
}

/// Saves `menu` as `ids`.
fn save(menu: &str, ids: Vec<Id>, cx: &mut Cx<'_, '_>) {
    if let Ok(menu) = Id::parse(menu) {
        cx.command(Command::SetMenu(menu, Some(ids)));
    }
}

/// A full screen with a header and a list, whose first rows may be moved
/// by dragging.
struct Frame {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    reorder: Reorder,
}

impl Frame {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height - top);
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            reorder: Reorder::default(),
        }
    }

    /// Draws the rows; the first `movable` with a grip.
    fn draw(
        &mut self,
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        title: &str,
        rows: &[(String, String)],
        movable: usize,
    ) {
        let style = self.style;
        self.header.draw(
            canvas,
            env,
            &style,
            title,
            None,
            self.list.paging(rows.len()),
        );
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| {
                let (label, value) = &rows[index];
                let rect = if index < movable {
                    Reorder::draw_grip(canvas, env, &style, rect, focused);
                    Reorder::content(rect)
                } else {
                    rect
                };
                ui::draw_value_row(canvas, env, &style, rect, focused, label, value);
            },
        );
        self.reorder.draw(canvas, &style, &self.list);
    }

    /// Follows a finger dragging one of the first `movable` rows: where
    /// it was let go, the row moved (`from`, `to`).
    fn on_pointer(
        &mut self,
        event: &PointerEvent,
        movable: usize,
        cx: &mut Cx<'_, '_>,
    ) -> (Handled, Option<(usize, usize)>) {
        let Some(step) = self.reorder.on_pointer(event, &self.list, movable) else {
            return (Handled::No, None);
        };
        if step != ReorderStep::Same {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        match step {
            ReorderStep::Dropped { from, to } if from != to => {
                self.list.focus = to;
                (Handled::Yes, Some((from, to)))
            }
            _ => (Handled::Yes, None),
        }
    }

    /// Moves the focus for navigation actions; `Some(index)` to choose an
    /// entry, `None` for other actions.
    fn on_action(&mut self, action: &Id, len: usize, cx: &mut Cx<'_, '_>) -> Option<Handled> {
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
        } else {
            return None;
        };
        if self.list.move_focus(delta, len) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Some(Handled::Yes)
    }

    /// The row a tap chose; header taps run their action.
    fn on_tap(&mut self, gesture: Gesture, len: usize, cx: &mut Cx<'_, '_>) -> Option<usize> {
        if gesture.kind != GestureKind::Tap {
            return None;
        }
        let paging = self.list.paging(len);
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return None;
        }
        let index = self.list.hit(gesture.start, len)?;
        if index != self.list.focus {
            self.list.focus = index;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Some(index)
    }
}

// ---- The list of menus --------------------------------------------------

struct MenusScreen {
    /// Menus with their names; submenus follow their menu as
    /// "Menu › Submenu".
    menus: Vec<(Id, String)>,
    frame: Frame,
}

impl MenusScreen {
    fn open(env: &Env<'_>) -> Self {
        let all = env.menus.menu_ids();
        let is_sub = |m: &Id| {
            all.iter()
                .any(|other| env.menus.ids(other.as_str()).contains(&m))
        };
        let mut roots: Vec<(Id, String)> = all
            .iter()
            .filter(|m| !is_sub(m))
            .map(|m| ((*m).clone(), env.i18n.tr(m.as_str()).to_string()))
            .collect();
        roots.sort_by_key(|(_, name)| name.to_lowercase());
        let mut menus = Vec::new();
        for (root, name) in roots {
            add_with_submenus(env, root, name, &mut menus);
        }
        Self {
            menus,
            frame: Frame::new(env),
        }
    }

    fn rows(&self, env: &Env<'_>) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = self
            .menus
            .iter()
            .map(|(m, name)| {
                let changed = env.menus.is_customized(m.as_str());
                let value = if changed {
                    env.i18n.tr("menu-editor.changed")
                } else {
                    ""
                };
                (name.clone(), alloc::format!("{value} ›"))
            })
            .collect();
        if self
            .menus
            .iter()
            .any(|(m, _)| env.menus.is_customized(m.as_str()))
        {
            rows.push((
                env.i18n.tr("menu-editor.reset-all").to_string(),
                String::new(),
            ));
        }
        rows
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match self.menus.get(index) {
            Some((menu, _)) => cx.command(Command::OpenScreen(
                ENTRIES_SCREEN,
                Params::new().with(MENU, menu.as_str().to_string()),
            )),
            // Everything as the plugins define it.
            None => {
                for (menu, _) in &self.menus {
                    if env.menus.is_customized(menu.as_str()) {
                        cx.command(Command::SetMenu(menu.clone(), None));
                    }
                }
                self.frame.list.focus = 0;
            }
        }
    }
}

/// Adds `menu` and, after it, its submenus.
fn add_with_submenus(env: &Env<'_>, menu: Id, name: String, out: &mut Vec<(Id, String)>) {
    if out.iter().any(|(m, _)| *m == menu) {
        return;
    }
    let subs: Vec<Id> = entries(env, menu.as_str())
        .into_iter()
        .filter(|id| is_menu(env, id.as_str()))
        .collect();
    out.push((menu, name.clone()));
    for sub in subs {
        let sub_name = alloc::format!("{name} › {}", name_of_menu(env, &sub));
        add_with_submenus(env, sub, sub_name, out);
    }
}

fn name_of_menu(env: &Env<'_>, menu: &Id) -> String {
    env.i18n.tr(menu.as_str()).to_string()
}

impl Screen for MenusScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let rows = self.rows(env);
        let title = env.i18n.tr("menu-editor.title");
        self.frame.draw(canvas, env, title, &rows, 0);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows(cx.env()).len();
        if *action == actions::CONFIRM {
            self.choose(self.frame.list.focus, cx);
            return Handled::Yes;
        }
        self.frame.on_action(action, len, cx).unwrap_or(Handled::No)
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows(cx.env()).len();
        match self.frame.on_tap(gesture, len, cx) {
            Some(index) => {
                self.choose(index, cx);
                Handled::Yes
            }
            None => tap_result(gesture),
        }
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == quireink_sdk::builtin::events::MENU_CHANGED {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }
}

// ---- The entries of one menu --------------------------------------------

struct EntriesScreen {
    menu: String,
    frame: Frame,
}

impl EntriesScreen {
    fn open(env: &Env<'_>, params: &Params) -> Self {
        Self {
            menu: params.get(MENU).unwrap_or_default().to_string(),
            frame: Frame::new(env),
        }
    }

    /// The entries, then *Add* and, if the menu was changed, *Reset*.
    fn rows(&self, env: &Env<'_>) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = entries(env, &self.menu)
            .iter()
            .map(|id| {
                let more = if is_menu(env, id.as_str()) { "›" } else { "" };
                (name(env, id.as_str()), more.to_string())
            })
            .collect();
        rows.push((env.i18n.tr("menu-editor.add").to_string(), "›".to_string()));
        if env.menus.is_customized(&self.menu) {
            rows.push((env.i18n.tr("menu-editor.reset").to_string(), String::new()));
        }
        rows
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let count = entries(&env, &self.menu).len();
        let params = Params::new().with(MENU, self.menu.clone());
        if index < count {
            let params = params.with(INDEX, alloc::format!("{index}"));
            cx.command(Command::OpenScreen(ENTRY_SCREEN, params));
        } else if index == count {
            cx.command(Command::OpenScreen(ADD_SCREEN, params));
        } else if let Ok(menu) = Id::parse(&self.menu) {
            cx.command(Command::SetMenu(menu, None));
            self.frame.list.focus = 0;
        }
    }
}

impl Screen for EntriesScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let rows = self.rows(env);
        let title = env.i18n.tr(&self.menu).to_string();
        let movable = entries(env, &self.menu).len();
        self.frame.draw(canvas, env, &title, &rows, movable);
    }

    /// Entries are moved by dragging their grip.
    fn on_pointer(&mut self, event: &PointerEvent, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let mut ids = entries(&env, &self.menu);
        let (handled, moved) = self.frame.on_pointer(event, ids.len(), cx);
        if let Some((from, to)) = moved {
            ui::reorder::moved(&mut ids, from, to);
            save(&self.menu, ids, cx);
        }
        handled
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows(cx.env()).len();
        if *action == actions::CONFIRM {
            self.choose(self.frame.list.focus, cx);
            return Handled::Yes;
        }
        self.frame.on_action(action, len, cx).unwrap_or(Handled::No)
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows(cx.env()).len();
        match self.frame.on_tap(gesture, len, cx) {
            Some(index) => {
                self.choose(index, cx);
                Handled::Yes
            }
            None => tap_result(gesture),
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == MOVED || *event == quireink_sdk::builtin::events::MENU_CHANGED {
            // The focus follows a moved entry.
            if let Some(focus) = params.get(INDEX).and_then(|i| i.parse().ok()) {
                self.frame.list.focus = focus;
            }
            let len = self.rows(cx.env()).len();
            self.frame.list.focus = self.frame.list.focus.min(len - 1);
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }
}

// ---- What to do with an entry -------------------------------------------

/// An overlay with what can be done with one entry: move it up or down,
/// or remove it (refused when that would lock one out).
struct EntryScreen {
    menu: String,
    index: usize,
    /// Shown instead of the options when a removal was refused.
    refused: bool,
    style: UiStyle,
    region: Rect,
    list: ListView,
}

const OPTIONS: [&str; 4] = [
    "menu-editor.up",
    "menu-editor.down",
    "menu-editor.remove",
    "menu-editor.cancel",
];

impl EntryScreen {
    fn open(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let height = (style.row * (OPTIONS.len() as u32 + 1) + style.gap * 2) as u16;
        let region = Rect::new(0, display.height - height, display.width, height);
        let list_area = Rect::new(
            0,
            region.y + style.row as u16 + style.gap as u16,
            display.width,
            height - style.row as u16 - style.gap as u16,
        );
        Self {
            menu: params.get(MENU).unwrap_or_default().to_string(),
            index: params.get(INDEX).and_then(|i| i.parse().ok()).unwrap_or(0),
            refused: false,
            list: ListView::new(list_area, style.row).focus_on_keys(env),
            style,
            region,
        }
    }

    fn choose(&mut self, option: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if self.refused {
            cx.command(Command::Back);
            return;
        }
        let mut ids = entries(&env, &self.menu);
        let i = self.index;
        if i >= ids.len() {
            cx.command(Command::Back);
            return;
        }
        let to = match option {
            0 if i > 0 => {
                ids.swap(i, i - 1);
                i - 1
            }
            1 if i + 1 < ids.len() => {
                ids.swap(i, i + 1);
                i + 1
            }
            2 => {
                let mut fewer = ids.clone();
                fewer.remove(i);
                if !keeps_access(&env, &self.menu, &fewer) {
                    self.refused = true;
                    cx.request_redraw();
                    cx.request_refresh(RefreshMode::Fast);
                    return;
                }
                ids = fewer;
                i.saturating_sub(1)
            }
            _ => {
                cx.command(Command::Back);
                return;
            }
        };
        cx.command(Command::Back);
        save(&self.menu, ids, cx);
        // Tells the list where the entry went, so it stays focused.
        cx.command(Command::Emit(
            MOVED,
            Params::new().with(INDEX, alloc::format!("{to}")),
        ));
    }
}

impl Screen for EntryScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            (style.line * 2).max(2),
            Color::BLACK,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        let id = entries(env, &self.menu).get(self.index).cloned();
        let title = id.map_or(String::new(), |id| name(env, id.as_str()));
        let title_area = Rect::new(r.x, r.y + style.gap as u16, r.width, style.row as u16);
        let x = style.margin as f32;
        let title = ui::fit(&font, style.title, &title, f32::from(r.width) - 2.0 * x);
        ui::draw_label(
            canvas,
            &font,
            style.title,
            x,
            title_area,
            &title,
            Color::BLACK,
        );
        if self.refused {
            let message = env.i18n.tr("menu-editor.locked");
            ui::draw_message(canvas, env, &style, self.list.area, message);
            return;
        }
        self.list.draw(
            canvas,
            &style,
            OPTIONS.len(),
            |canvas, index, rect, focused| {
                let label = env.i18n.tr(OPTIONS[index]);
                let color = if focused { Color::WHITE } else { Color::BLACK };
                ui::draw_label(canvas, &font, style.text, x, rect, label, color);
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if *a == actions::CONFIRM {
            self.choose(self.list.focus, cx);
            return Handled::Yes;
        }
        let delta = if [
            actions::UP,
            actions::LEFT,
            actions::PREVIOUS,
            actions::PREV_PAGE,
        ]
        .contains(a)
        {
            -1
        } else if [
            actions::DOWN,
            actions::RIGHT,
            actions::NEXT,
            actions::NEXT_PAGE,
        ]
        .contains(a)
        {
            1
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, OPTIONS.len()) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap || !self.region.contains(gesture.start) {
            return Handled::No;
        }
        if self.refused {
            cx.command(Command::Back);
        } else if let Some(index) = self.list.hit(gesture.start, OPTIONS.len()) {
            self.list.focus = index;
            self.choose(index, cx);
        }
        Handled::Yes
    }
}

// ---- Adding an entry ----------------------------------------------------

/// Everything that can go into a menu and is not in it yet: actions,
/// settings and other menus, by name.
struct AddScreen {
    menu: String,
    /// Ids with their names, sorted by name.
    candidates: Vec<(Id, String)>,
    frame: Frame,
}

impl AddScreen {
    fn open(env: &Env<'_>, params: &Params) -> Self {
        let menu = params.get(MENU).unwrap_or_default().to_string();
        let present = entries(env, &menu);
        let mut candidates: Vec<(Id, String)> = env
            .actions
            .iter()
            .map(|a| a.id.clone())
            .chain(env.settings.specs().map(|s| s.id.clone()))
            .chain(env.menus.menu_ids().into_iter().cloned())
            .filter(|id| !present.contains(id) && id.as_str() != menu)
            .map(|id| {
                let name = name(env, id.as_str());
                (id, name)
            })
            .collect();
        candidates.sort_by_key(|c| c.1.to_lowercase());
        candidates.dedup_by(|a, b| a.0 == b.0);
        Self {
            menu,
            candidates,
            frame: Frame::new(env),
        }
    }

    fn rows(&self, env: &Env<'_>) -> Vec<(String, String)> {
        self.candidates
            .iter()
            .map(|(id, name)| {
                let more = if is_menu(env, id.as_str()) { "›" } else { "" };
                (name.clone(), more.to_string())
            })
            .collect()
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some((id, _)) = self.candidates.get(index) else {
            return;
        };
        let mut ids = entries(&env, &self.menu);
        ids.push(id.clone());
        cx.command(Command::Back);
        save(&self.menu, ids, cx);
    }
}

impl Screen for AddScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let rows = self.rows(env);
        let title = env.i18n.tr("menu-editor.add").to_string();
        self.frame.draw(canvas, env, &title, &rows, 0);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.candidates.len();
        if *action == actions::CONFIRM {
            self.choose(self.frame.list.focus, cx);
            return Handled::Yes;
        }
        self.frame.on_action(action, len, cx).unwrap_or(Handled::No)
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.candidates.len();
        match self.frame.on_tap(gesture, len, cx) {
            Some(index) => {
                self.choose(index, cx);
                Handled::Yes
            }
            None => tap_result(gesture),
        }
    }
}

/// Taps are handled (also on the header); other gestures go on to the key
/// map, e.g. swipes to turn pages.
fn tap_result(gesture: Gesture) -> Handled {
    if gesture.kind == GestureKind::Tap {
        Handled::Yes
    } else {
        Handled::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reachability_follows_submenus_and_stops_at_cycles() {
        let menu = |name: &str| -> Vec<Id> {
            match name {
                "a.root" => alloc::vec![Id::new_static("a.x"), Id::new_static("a.sub")],
                "a.sub" => alloc::vec![Id::new_static("a.root"), Id::new_static("core.settings")],
                _ => Vec::new(),
            }
        };
        assert!(reaches(
            &menu,
            "a.root",
            &actions::SETTINGS,
            &mut Vec::new()
        ));
        assert!(!reaches(&menu, "a.root", &OPEN, &mut Vec::new()));
    }

    #[test]
    fn translations_are_complete() {
        use quireink_i18n::{Catalog, check};
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}
