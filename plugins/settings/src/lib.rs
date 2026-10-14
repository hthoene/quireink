//! The settings screens, generated from the settings every plugin declares
//! (`Registrar::add_setting`) — no plugin needs its own settings UI.
//!
//! - The main page lists the setting groups, pages other plugins add to
//!   the `core.settings-pages` menu (e.g. the plugin manager), and a
//!   switch for advanced settings.
//! - A group page lists its settings with their values and units, each
//!   with its description below it, so nothing has to be tapped (and
//!   changed) to find out what it does.
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

use quireink_sdk::builtin::{actions, events, groups, menus, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Level, Manifest, Params,
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
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Whether a setting is shown with the current level.
fn visible(env: &Env<'_>, spec: &SettingSpec) -> bool {
    spec.level == Level::Basic || env.settings.bool(ADVANCED.as_str())
}

/// The header, list area and list for a screen; rows have notes if
/// `noted`.
fn frame(env: &Env<'_>, noted: bool) -> (UiStyle, Rect, ListView) {
    let style = UiStyle::new(env);
    let display = env.device.display;
    let header = Rect::new(0, 0, display.width, style.header as u16);
    let top = header.height + style.gap as u16;
    let height = display.height.saturating_sub(top);
    let area = Rect::new(0, top, display.width, height);
    let row = if noted { style.noted_row() } else { style.row };
    (style, header, ListView::new(area, row))
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

fn page_note(env: &Env<'_>, list: &ListView, len: usize) -> Option<String> {
    (list.pages(len) > 1).then(|| {
        env.i18n.format(
            "settings.page",
            &[
                ("page", Arg::Int(list.page() as i64 + 1)),
                ("pages", Arg::Int(list.pages(len) as i64)),
            ],
        )
    })
}

// ---- Main page: groups -------------------------------------------------

#[derive(Debug)]
struct GroupList {
    /// Groups with visible settings, by order.
    groups: Vec<(Id, String)>,
    /// Pages of other plugins: `(action, title)`.
    pages: Vec<(Id, String)>,
    style: UiStyle,
    header: Rect,
    list: ListView,
}

impl GroupList {
    fn new(env: &Env<'_>) -> Self {
        let (style, header, list) = frame(env, false);
        let groups = env
            .settings
            .groups()
            .filter(|group| {
                env.settings
                    .specs()
                    .any(|spec| env.settings.group_of(spec) == group.id && visible(env, spec))
            })
            .map(|group| (group.id.clone(), env.i18n.tr(&group.title).to_string()))
            .collect();
        let pages = env
            .menus
            .items(menus::SETTINGS.as_str())
            .iter()
            .map(|item| (item.id.clone(), env.i18n.tr(item.title).to_string()))
            .collect();
        Self {
            groups,
            pages,
            style,
            header,
            list,
        }
    }

    /// Groups, pages, then the switch for advanced settings.
    fn len(&self) -> usize {
        self.groups.len() + self.pages.len() + 1
    }

    /// The group or page at `index`; `None` for the advanced switch.
    fn link(&self, index: usize) -> Option<&(Id, String)> {
        self.groups
            .get(index)
            .or_else(|| self.pages.get(index - self.groups.len()))
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some((id, _)) = self.groups.get(index) {
            cx.command(Command::OpenScreen(
                GROUP_SCREEN,
                Params::new().with(ID, id.as_str().to_string()),
            ));
        } else if let Some((action, _)) = self.link(index) {
            cx.command(Command::RunAction(action.clone()));
        } else {
            let advanced = cx.env().settings.bool(ADVANCED.as_str());
            cx.command(Command::SetSetting(ADVANCED, SettingValue::Bool(!advanced)));
        }
    }
}

impl Screen for GroupList {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        let note = page_note(env, &self.list, len);
        ui::draw_header(
            canvas,
            env,
            &style,
            self.header,
            env.i18n.tr("settings.title"),
            note.as_deref(),
        );
        let advanced = if env.settings.bool(ADVANCED.as_str()) {
            env.i18n.tr("settings.on")
        } else {
            env.i18n.tr("settings.off")
        };
        let (groups, pages) = (&self.groups, &self.pages);
        let link = |index: usize| {
            groups
                .get(index)
                .or_else(|| pages.get(index - groups.len()))
        };
        self.list.draw(
            canvas,
            &style,
            len,
            |canvas, index, rect, focused| match link(index) {
                Some((_, title)) => {
                    ui::draw_value_row(canvas, env, &style, rect, focused, title, "›");
                }
                None => ui::draw_value_row(
                    canvas,
                    env,
                    &style,
                    rect,
                    focused,
                    env.i18n.tr("settings.advanced"),
                    advanced,
                ),
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len();
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
        match self.list.hit(gesture.start, self.len()) {
            Some(index) => {
                self.list.focus = index;
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
            let focus = self.list.focus;
            *self = Self::new(&env);
            self.list.focus = focus.min(self.len() - 1);
            cx.request_redraw();
        }
    }
}

// ---- Group page: settings ----------------------------------------------

#[derive(Debug)]
struct GroupPage {
    group: Id,
    title: String,
    specs: Vec<SettingSpec>,
    style: UiStyle,
    header: Rect,
    list: ListView,
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
        let noted = specs.iter().any(|spec| spec.description.is_some());
        let (style, header, list) = frame(env, noted);
        Self {
            group,
            title,
            specs,
            style,
            header,
            list,
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

    fn focused(&self) -> Option<&SettingSpec> {
        self.specs.get(self.list.focus)
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
                SettingValue::Int((v + step).clamp(*min, *max))
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
            // Text needs a keyboard (planned); edit it in quireink.ini.
            _ => {}
        }
    }
}

impl Screen for GroupPage {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.specs.len();
        let note = page_note(env, &self.list, len);
        ui::draw_header(
            canvas,
            env,
            &style,
            self.header,
            &self.title,
            note.as_deref(),
        );
        let specs = &self.specs;
        let editing = self.editing;
        let noted = self.noted;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let spec = &specs[index];
                let title = env.i18n.tr(&spec.title);
                let note = spec.description.as_deref().map_or("", |d| env.i18n.tr(d));
                let mut value = ui::setting_value(env, spec);
                if focused && matches!(spec.kind, SettingKind::Int { .. }) {
                    let area = if noted {
                        ui::title_area(&style, rect)
                    } else {
                        rect
                    };
                    ui::draw_stepper(canvas, env, &style, area, title, &value, editing);
                    if noted && !note.is_empty() {
                        ui::draw_note(canvas, env, &style, rect, true, note);
                    }
                    return;
                }
                if matches!(spec.kind, SettingKind::Choice(_) | SettingKind::Language) {
                    value.push_str(" ›");
                }
                ui::draw_noted_row(canvas, env, &style, rect, focused, title, &value, note);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let len = self.specs.len();
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
        if let Some(change) = navigate(&mut self.list, len, action) {
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
        let Some(index) = self.list.hit(gesture.start, self.specs.len()) else {
            return Handled::No;
        };
        let env = *cx.env();
        let is_number = matches!(
            self.specs.get(index).map(|s| &s.kind),
            Some(SettingKind::Int { .. })
        );
        if is_number {
            // The first tap selects a number; then its − and + buttons
            // change it, so a value never changes by accident.
            if index != self.list.focus {
                self.list.focus = index;
                self.editing = false;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
                return Handled::Yes;
            }
            let Some(spec) = self.focused() else {
                return Handled::Yes;
            };
            let rect = self.title_area(self.list.row_rect(index % self.list.rows()));
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
        if index != self.list.focus {
            self.list.focus = index;
            self.editing = false;
            cx.request_redraw();
        }
        self.activate(&env, 1, cx);
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED {
            let env = *cx.env();
            let (focus, editing) = (self.list.focus, self.editing);
            *self = Self::new(&env, self.group.clone());
            self.list.focus = focus.min(self.specs.len().saturating_sub(1));
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
    header: Rect,
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
        let note = page_note(env, &self.list, len);
        ui::draw_header(canvas, env, &style, self.header, title, note.as_deref());
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
