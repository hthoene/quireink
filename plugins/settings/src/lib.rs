//! The settings screens, generated from the settings every plugin declares
//! (`Registrar::add_setting`), so no plugin needs its own settings UI.
//!
//! - The main page lists the setting groups and pages other plugins add
//!   to the `core.settings-pages` menu (e.g. the plugin manager) with
//!   their icons, under the headings of the sections in that menu (by
//!   order), and a switch for advanced settings.
//! - A group page lists its settings with their values and units, each
//!   with its description below it, so nothing has to be tapped (and
//!   changed) to find out what it does; settings that name a section
//!   are listed under its heading.
//! - Choices and languages open a list to pick from.
//!
//! Changes apply immediately and are saved to `quireink.ini` by the
//! kernel.
//!
//! Buttons: move to an item, *confirm* opens a group, toggles a switch,
//! opens a choice or starts editing a number; while editing, left/right
//! (or up/down, page buttons) change it and *confirm*/*back* finish.
//! Touch: tap an item to open, toggle or choose it; a tapped number shows
//! − and + buttons that change it.
//!
//! The main page is bound to the `core.settings` slot.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, groups, menus, params, setting_sections, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Level, Manifest, Params,
    Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("settings");
/// The main page: setting groups.
pub const SCREEN: Id = Id::new_static("settings.list");
/// The settings of one group; parameter `id`.
pub const GROUP_SCREEN: Id = Id::new_static("settings.group");
/// Picks a value of a choice or language setting; parameter `id`.
pub const CHOOSE_SCREEN: Id = Id::new_static("settings.choose");
/// Show advanced settings too.
pub const ADVANCED: Id = Id::new_static("settings.advanced");
/// Emitted by the keyboard with a text setting's new value (parameters
/// `id` and `text`).
const TEXT_ENTERED: Id = Id::new_static("settings.text-entered");

const ID: &str = "id";

/// The settings plugin.
#[derive(Debug, Default)]
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Settings"),
            description: Cow::Borrowed(
                "The settings pages, built from the settings of every plugin.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("settings.title"),
            factory: Box::new(|env, _| Box::new(GroupList::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: GROUP_SCREEN,
            title: Cow::Borrowed("settings.title"),
            factory: Box::new(|env, params| {
                let group = params.get(ID).and_then(|id| Id::parse(id).ok());
                Box::new(GroupPage::new(env, group.unwrap_or(groups::OTHER)))
            }),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOOSE_SCREEN,
            title: Cow::Borrowed("settings.title"),
            factory: Box::new(|env, params| {
                let id = params.get(ID).unwrap_or_default();
                Box::new(Chooser::new(env, id))
            }),
        })?;
        r.bind_slot(slots::SETTINGS, SCREEN, Priority::DEFAULT)?;
        r.add_setting(
            SettingSpec::new(
                ADVANCED,
                "settings.advanced",
                SettingKind::Bool,
                SettingValue::Bool(false),
            )
            .group(groups::SYSTEM)
            .description("settings.advanced.help"),
        )?;
        r.subscribe(
            TEXT_ENTERED,
            Box::new(|env: &Env<'_>, params: &Params, out: &mut Vec<Command>| {
                let id = params.get(ID).and_then(|id| Id::parse(id).ok());
                if let (Some(id), Some(text)) = (id, params.get(params::TEXT))
                    && env.settings.get(id.as_str()).is_some()
                {
                    out.push(Command::SetSetting(
                        id,
                        SettingValue::Text(Cow::Owned(text.trim().to_string())),
                    ));
                }
            }),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Whether a setting is shown with the current level.
fn visible(env: &Env<'_>, spec: &SettingSpec) -> bool {
    // "Only this book" and the like mean nothing outside a scope.
    let scope_only = spec.id == quireink_sdk::builtin::settings::SCOPE_ONLY;
    if scope_only && env.settings.scope().is_none() {
        return false;
    }
    // Not for this device (e.g. reading aloud without a speaker).
    if !env.menus.available(spec.id.as_str()) {
        return false;
    }
    spec.level == Level::Basic || env.settings.bool(ADVANCED.as_str())
}

/// The header, list area and list for a screen; rows have notes if
/// `noted`.
fn frame(env: &Env<'_>, noted: bool) -> (UiStyle, ui::Header, ListView) {
    let (style, header, area) = page(env);
    let row = if noted { style.noted_row() } else { style.row };
    (style, header, ListView::new(area, row).focus_on_keys(env))
}

/// The style, header and the area below it for the list of a page.
fn page(env: &Env<'_>) -> (UiStyle, ui::Header, Rect) {
    let style = UiStyle::new(env);
    let display = env.device.display;
    let header = ui::Header::new(env, &style, true);
    let top = header.area.height + style.gap as u16;
    let height = display.height.saturating_sub(top);
    (style, header, Rect::new(0, top, display.width, height))
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

/// Moves `list` by the navigation actions; `None` if `action` is another.
fn navigate(list: &mut ListView, len: usize, action: &Id) -> Option<ListChange> {
    let rows = list.rows() as isize;
    let delta =
        if *action == actions::UP || *action == actions::LEFT || *action == actions::PREVIOUS {
            -1
        } else if *action == actions::DOWN || *action == actions::RIGHT || *action == actions::NEXT
        {
            1
        } else if *action == actions::NEXT_PAGE {
            rows
        } else if *action == actions::PREV_PAGE {
            -rows
        } else {
            return None;
        };
    Some(list.move_focus(delta, len))
}

/// Moves the focus of `rows` (lists with headings) by the navigation
/// actions; `None` if `action` is another. The first press on a touch
/// screen only shows the focus (`keys`), which taps hide.
fn navigate_rows(rows: &mut ui::Rows, keys: &mut bool, action: &Id) -> Option<ListChange> {
    let forward =
        if *action == actions::UP || *action == actions::LEFT || *action == actions::PREVIOUS {
            Some(false)
        } else if *action == actions::DOWN || *action == actions::RIGHT || *action == actions::NEXT
        {
            Some(true)
        } else {
            None
        };
    let page = if *action == actions::NEXT_PAGE {
        Some(true)
    } else if *action == actions::PREV_PAGE {
        Some(false)
    } else {
        None
    };
    if let Some(forward) = page {
        // Pages turn whether the focus shows or not.
        return Some(rows.turn(forward));
    }
    let forward = forward?;
    if !*keys {
        *keys = true;
        return Some(ListChange::Focus);
    }
    Some(rows.step(forward))
}

/// The page shown in a header: its number and the number of pages.
fn paging(rows: &ui::Rows) -> Option<(usize, usize)> {
    Some(rows.paging())
}

// ---- Main page: groups -------------------------------------------------

/// What an entry of the main page opens.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Link {
    Group(Id),
    /// A page of another plugin: its action.
    Page(Id),
    /// The switch for advanced settings.
    Advanced,
}

#[derive(Debug)]
struct Entry {
    /// `None` for a section's heading.
    link: Option<Link>,
    title: String,
    icon: Option<String>,
}

#[derive(Debug)]
struct GroupList {
    /// Groups with visible settings and pages of other plugins, by order
    /// under the headings of their sections, then the advanced switch.
    entries: Vec<Entry>,
    style: UiStyle,
    header: ui::Header,
    rows: ui::Rows,
    /// The focus shows (buttons were used).
    keys: bool,
    /// Room for icons at the start of rows.
    icon_room: u16,
}

impl GroupList {
    fn new(env: &Env<'_>) -> Self {
        let (style, header, area) = page(env);
        let settings_menu = menus::SETTINGS;
        let menu = settings_menu.as_str();
        // (order, headings before entries of the same order, entry)
        let mut placed: Vec<(i32, bool, Entry)> = env
            .menus
            .items(menu)
            .iter()
            .map(|item| {
                let order = env.menus.order_of(menu, item.id.as_str());
                (
                    order.map_or(i32::from(i16::MAX), i32::from),
                    !item.section,
                    Entry {
                        link: (!item.section).then(|| Link::Page(item.id.clone())),
                        title: env.i18n.tr(item.title).to_string(),
                        icon: item.icon.map(String::from),
                    },
                )
            })
            .collect();
        placed.extend(
            env.settings
                .groups()
                .filter(|group| {
                    env.settings
                        .specs()
                        .any(|spec| env.settings.group_of(spec) == group.id && visible(env, spec))
                })
                .map(|group| {
                    (
                        group.order,
                        true,
                        Entry {
                            link: Some(Link::Group(group.id.clone())),
                            title: env.i18n.tr(&group.title).to_string(),
                            icon: env.menus.icon(group.id.as_str()).map(String::from),
                        },
                    )
                }),
        );
        placed.push((
            i32::MAX,
            true,
            Entry {
                link: Some(Link::Advanced),
                title: env.i18n.tr("settings.advanced").to_string(),
                icon: Some(String::from("eye")),
            },
        ));
        placed.sort_by_key(|(order, entry, _)| (*order, *entry));
        let mut entries: Vec<Entry> = placed.into_iter().map(|(_, _, entry)| entry).collect();
        // Headings with nothing under them go.
        let empty: Vec<bool> = (0..entries.len())
            .map(|i| {
                entries[i].link.is_none()
                    && entries.get(i + 1).is_none_or(|next| next.link.is_none())
            })
            .collect();
        let mut index = 0;
        entries.retain(|_| {
            index += 1;
            !empty[index - 1]
        });
        let heading = ui::heading_height(env, &style);
        let heights = entries
            .iter()
            .map(|e| {
                if e.link.is_none() {
                    heading
                } else {
                    style.row as u16
                }
            })
            .collect();
        let headings = entries.iter().map(|e| e.link.is_none()).collect();
        let icon_room = ui::icon_room(
            env,
            &style,
            entries.iter().any(|e| e.link.is_some() && e.icon.is_some()),
        );
        Self {
            entries,
            style,
            header,
            rows: ui::Rows::new(area, heights, headings),
            keys: !env.device.input.touch,
            icon_room,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        match self.entries.get(index).and_then(|e| e.link.as_ref()) {
            Some(Link::Group(id)) => cx.command(Command::OpenScreen(
                GROUP_SCREEN,
                Params::new().with(ID, id.as_str().to_string()),
            )),
            Some(Link::Page(action)) => cx.command(Command::RunAction(action.clone())),
            Some(Link::Advanced) => {
                let advanced = cx.env().settings.bool(ADVANCED.as_str());
                cx.command(Command::SetSetting(ADVANCED, SettingValue::Bool(!advanced)));
            }
            None => {}
        }
    }
}

impl Screen for GroupList {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("settings.title"),
            None,
            paging(&self.rows),
        );
        let (entries, room) = (&self.entries, self.icon_room);
        self.rows
            .draw(canvas, &style, self.keys, |canvas, index, rect, focused| {
                let entry = &entries[index];
                let Some(link) = &entry.link else {
                    ui::draw_heading(canvas, env, &style, rect, &entry.title);
                    return;
                };
                let ink = if focused { Color::WHITE } else { Color::BLACK };
                let icon = entry.icon.as_deref();
                let rect = ui::draw_row_icon(canvas, env, &style, rect, room, icon, ink);
                if *link == Link::Advanced {
                    let on = env.settings.bool(ADVANCED.as_str());
                    ui::draw_switch_row(canvas, env, &style, rect, focused, &entry.title, "", on);
                } else {
                    ui::draw_value_row(canvas, env, &style, rect, focused, &entry.title, "›");
                }
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if let Some(change) = navigate_rows(&mut self.rows, &mut self.keys, action) {
            moved(change, cx);
        } else if *action == actions::CONFIRM {
            self.choose(self.rows.focus, cx);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging(&self.rows));
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.rows.hit(gesture.start) {
            Some(index) => {
                self.rows.focus = index;
                self.keys = !cx.env().device.input.touch;
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED {
            // Language, UI size or the advanced switch may have changed.
            let env = *cx.env();
            let (focus, keys) = (self.rows.focus, self.keys);
            *self = Self::new(&env);
            self.keys = keys;
            refocus(&mut self.rows, focus);
            cx.request_redraw();
        }
    }
}

/// Puts the focus of rebuilt `rows` back at `focus`, or the nearest entry.
fn refocus(rows: &mut ui::Rows, focus: usize) {
    if rows.is_empty() {
        return;
    }
    rows.focus = focus.min(rows.len() - 1);
    if rows.is_heading(rows.focus) && rows.step(true) == ListChange::None {
        rows.step(false);
    }
}

// ---- Group page: settings ----------------------------------------------

/// A row of a group page.
#[derive(Debug)]
enum Row {
    /// A section's heading: its title.
    Heading(String),
    Setting(Box<SettingSpec>),
}

#[derive(Debug)]
struct GroupPage {
    group: Id,
    title: String,
    entries: Vec<Row>,
    style: UiStyle,
    header: ui::Header,
    rows: ui::Rows,
    /// The focus shows (buttons were used, or a number was tapped).
    keys: bool,
    /// The focused number is being edited.
    editing: bool,
    /// Rows have room for descriptions.
    noted: bool,
}

impl GroupPage {
    fn new(env: &Env<'_>, group: Id) -> Self {
        let title = env
            .settings
            .groups()
            .find(|g| g.id == group)
            .map_or_else(String::new, |g| env.i18n.tr(&g.title).to_string());
        let specs = env
            .settings
            .specs()
            .filter(|spec| env.settings.group_of(spec) == group && visible(env, spec))
            .cloned()
            .collect::<Vec<SettingSpec>>();
        // Sections: settings without one first, then the standard ones,
        // then the others as they come.
        let mut sections: Vec<Option<&str>> = Vec::new();
        for spec in &specs {
            let section = spec.section.as_deref();
            if !sections.contains(&section) {
                sections.push(section);
            }
        }
        let rank = |section: &Option<&str>| match section {
            None => 0,
            Some(key) => setting_sections::ORDER
                .iter()
                .position(|s| s == key)
                .map_or(setting_sections::ORDER.len() + 1, |p| p + 1),
        };
        sections.sort_by_key(rank);
        // Headings only where there is more than one part.
        let headed = sections.len() > 1;
        let mut entries = Vec::new();
        for section in &sections {
            if let Some(key) = section.filter(|_| headed) {
                entries.push(Row::Heading(env.i18n.tr(key).to_string()));
            }
            entries.extend(
                specs
                    .iter()
                    .filter(|spec| spec.section.as_deref() == *section)
                    .cloned()
                    .map(|spec| Row::Setting(Box::new(spec))),
            );
        }
        let noted = specs.iter().any(|spec| spec.description.is_some());
        let (style, header, area) = page(env);
        let heading = ui::heading_height(env, &style);
        let row = if noted { style.noted_row() } else { style.row } as u16;
        let heights = entries
            .iter()
            .map(|e| match e {
                Row::Heading(_) => heading,
                Row::Setting(_) => row,
            })
            .collect();
        let headings = entries
            .iter()
            .map(|e| matches!(e, Row::Heading(_)))
            .collect();
        Self {
            group,
            title,
            entries,
            style,
            header,
            rows: ui::Rows::new(area, heights, headings),
            keys: !env.device.input.touch,
            editing: false,
            noted,
        }
    }

    /// Where a row's title and value (or stepper) are drawn.
    fn title_area(&self, rect: Rect) -> Rect {
        if self.noted {
            ui::title_area(&self.style, rect)
        } else {
            rect
        }
    }

    fn spec(&self, index: usize) -> Option<&SettingSpec> {
        match self.entries.get(index) {
            Some(Row::Setting(spec)) => Some(spec),
            _ => None,
        }
    }

    fn focused(&self) -> Option<&SettingSpec> {
        self.spec(self.rows.focus)
    }

    /// Changes a number by `step` or toggles a switch.
    fn change(&self, env: &Env<'_>, step: i64, cx: &mut Cx<'_, '_>) {
        let Some(spec) = self.focused() else {
            return;
        };
        let current = env.settings.get(spec.id.as_str());
        let value = match (&spec.kind, current) {
            (SettingKind::Bool, Some(SettingValue::Bool(v))) => SettingValue::Bool(!v),
            (SettingKind::Int { min, max }, Some(SettingValue::Int(v))) => {
                SettingValue::Int((v + step * spec.step).clamp(*min, *max))
            }
            _ => return,
        };
        if Some(&value) != current {
            cx.command(Command::SetSetting(spec.id.clone(), value));
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    /// What confirming (or tapping) the focused setting does.
    fn activate(&mut self, env: &Env<'_>, step: i64, cx: &mut Cx<'_, '_>) {
        let Some(spec) = self.focused() else {
            return;
        };
        if let Some(editor) = &spec.editor {
            cx.command(Command::OpenScreen(
                editor.clone(),
                Params::new().with(ID, spec.id.as_str().to_string()),
            ));
            return;
        }
        match spec.kind {
            SettingKind::Bool => self.change(env, 1, cx),
            SettingKind::Int { .. } if self.editing => self.change(env, step, cx),
            SettingKind::Int { .. } => {
                self.editing = true;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
            SettingKind::Choice(_) | SettingKind::Language => {
                cx.command(Command::OpenScreen(
                    CHOOSE_SCREEN,
                    Params::new().with(ID, spec.id.as_str().to_string()),
                ));
            }
            SettingKind::Text => {
                cx.command(Command::OpenSlot(
                    slots::KEYBOARD,
                    Params::new()
                        .with(params::TITLE, env.i18n.tr(&spec.title).to_string())
                        .with(params::TEXT, ui::setting_value(env, spec))
                        .with(params::REPLY, TEXT_ENTERED.as_str())
                        .with(ID, spec.id.as_str().to_string()),
                ));
            }
            _ => {}
        }
    }
}

impl Screen for GroupPage {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header
            .draw(canvas, env, &style, &self.title, None, paging(&self.rows));
        let entries = &self.entries;
        let editing = self.editing;
        let noted = self.noted;
        self.rows
            .draw(canvas, &style, self.keys, |canvas, index, rect, focused| {
                let spec = match &entries[index] {
                    Row::Heading(title) => {
                        ui::draw_heading(canvas, env, &style, rect, title);
                        return;
                    }
                    Row::Setting(spec) => spec,
                };
                let title = env.i18n.tr(&spec.title);
                let note = spec.description.as_deref().map_or("", |d| env.i18n.tr(d));
                let note = if noted { note } else { "" };
                let mut value = ui::setting_value(env, spec);
                match spec.kind {
                    SettingKind::Int { .. } if focused => {
                        let area = if noted {
                            ui::title_area(&style, rect)
                        } else {
                            rect
                        };
                        ui::draw_stepper(canvas, env, &style, area, title, &value, editing);
                        if !note.is_empty() {
                            ui::draw_note(canvas, env, &style, rect, true, note);
                        }
                        return;
                    }
                    SettingKind::Bool if spec.editor.is_none() => {
                        let on = env.settings.bool(spec.id.as_str());
                        ui::draw_switch_row(canvas, env, &style, rect, focused, title, note, on);
                        return;
                    }
                    _ => {}
                }
                if spec.editor.is_some()
                    || matches!(spec.kind, SettingKind::Choice(_) | SettingKind::Language)
                {
                    value.push_str(" ›");
                }
                ui::draw_noted_row(canvas, env, &style, rect, focused, title, &value, note);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        if self.editing {
            let decrease = [
                actions::LEFT,
                actions::DOWN,
                actions::PREV_PAGE,
                actions::PREVIOUS,
            ];
            let increase = [
                actions::RIGHT,
                actions::UP,
                actions::NEXT_PAGE,
                actions::NEXT,
            ];
            if decrease.contains(action) {
                self.change(&env, -1, cx);
            } else if increase.contains(action) {
                self.change(&env, 1, cx);
            } else if *action == actions::CONFIRM || *action == actions::BACK {
                self.editing = false;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            } else {
                return Handled::No;
            }
            return Handled::Yes;
        }
        if let Some(change) = navigate_rows(&mut self.rows, &mut self.keys, action) {
            moved(change, cx);
        } else if *action == actions::CONFIRM {
            self.activate(&env, 1, cx);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging(&self.rows));
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.rows.hit(gesture.start) else {
            return Handled::No;
        };
        let env = *cx.env();
        let is_number = matches!(
            self.spec(index).map(|s| &s.kind),
            Some(SettingKind::Int { .. })
        );
        if is_number {
            // The first tap selects a number; then its − and + buttons
            // change it, so a value never changes by accident.
            if index != self.rows.focus || !self.keys {
                self.rows.focus = index;
                self.keys = true;
                self.editing = false;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
                return Handled::Yes;
            }
            let (Some(spec), Some(rect)) = (self.focused(), self.rows.rect_of(index)) else {
                return Handled::Yes;
            };
            let rect = self.title_area(rect);
            let value = ui::setting_value(&env, spec);
            if let Some((minus, plus)) = ui::stepper(&env, &self.style, rect, &value) {
                let x = gesture.start.x;
                let within = |r: Rect| x >= i32::from(r.x) && x < r.right() as i32;
                if within(minus) {
                    self.change(&env, -1, cx);
                } else if within(plus) {
                    self.change(&env, 1, cx);
                }
            }
            return Handled::Yes;
        }
        // Anything else acts at once; the focus hides again.
        let keys = !env.device.input.touch;
        if index != self.rows.focus || keys != self.keys {
            self.rows.focus = index;
            self.keys = keys;
            self.editing = false;
            cx.request_redraw();
        }
        self.activate(&env, 1, cx);
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED {
            let env = *cx.env();
            let (focus, keys, editing) = (self.rows.focus, self.keys, self.editing);
            *self = Self::new(&env, self.group.clone());
            refocus(&mut self.rows, focus);
            self.keys = keys;
            self.editing = editing;
            cx.request_redraw();
        }
    }
}

// ---- Choosing a value ---------------------------------------------------

#[derive(Debug)]
struct Chooser {
    spec: Option<SettingSpec>,
    /// (value, display name)
    options: Vec<(String, String)>,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
}

impl Chooser {
    fn new(env: &Env<'_>, id: &str) -> Self {
        let (style, header, mut list) = frame(env, false);
        let spec = env.settings.specs().find(|s| s.id.as_str() == id).cloned();
        let options: Vec<(String, String)> = match spec.as_ref().map(|s| (&s.kind, s)) {
            Some((SettingKind::Language, _)) => env
                .i18n
                .languages()
                .into_iter()
                .map(|(tag, name)| (tag.to_string(), name.to_string()))
                .collect(),
            Some((SettingKind::Choice(values), spec)) => values
                .iter()
                .map(|v| (v.to_string(), ui::option_name(env, spec, v)))
                .collect(),
            _ => Vec::new(),
        };
        let current = env.settings.text(id);
        list.focus = options.iter().position(|(v, _)| v == current).unwrap_or(0);
        Self {
            spec,
            options,
            style,
            header,
            list,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let (Some(spec), Some((value, _))) = (&self.spec, self.options.get(index)) else {
            return;
        };
        cx.command(Command::Back);
        cx.command(Command::SetSetting(
            spec.id.clone(),
            SettingValue::Text(Cow::Owned(value.clone())),
        ));
    }
}

impl Screen for Chooser {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.options.len();
        let title = self
            .spec
            .as_ref()
            .map_or("", |spec| env.i18n.tr(&spec.title));
        self.header
            .draw(canvas, env, &style, title, None, self.list.paging(len));
        let options = &self.options;
        let current = self
            .spec
            .as_ref()
            .map_or("", |spec| env.settings.text(spec.id.as_str()));
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let (value, name) = &options[index];
                let mark = if value == current { "•" } else { "" };
                ui::draw_value_row(canvas, env, &style, rect, focused, name, mark);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.options.len();
        if let Some(change) = navigate(&mut self.list, len, action) {
            moved(change, cx);
        } else if *action == actions::CONFIRM {
            self.choose(self.list.focus, cx);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = self.list.paging(self.options.len());
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.options.len()) {
            Some(index) => {
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
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
