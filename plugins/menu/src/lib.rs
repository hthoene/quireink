//! The menu overlay.
//!
//! Shows the menu named by the `menu` parameter at the bottom of the screen,
//! above whatever is open. Entries come from every plugin (see
//! `Registrar::add_menu_entry`) or from the user's `[menus]` definitions.
//! Choosing an action closes the menu and runs it. Settings show their
//! current value and change in place, so the menu stays open: switches
//! toggle, numbers have − and + buttons, choices open a list. An entry
//! whose action is named like a menu with entries opens that menu as a
//! submenu above this one.
//!
//! Bound to the `core.menu` slot; replace it to get a different menu style
//! (radial, side bar, …) for every menu at once. The same plugin shows
//! menus as panels of controls (`core.panel`, see `panel`).

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, menus, params, slots};
use quireink_sdk::ui::{self, ListChange, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params, Plugin,
    Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("menu");
pub const SCREEN: Id = Id::new_static("menu.list");
/// A menu as a panel of controls.
pub const PANEL: Id = Id::new_static("menu.panel");
/// Shows short messages (`core.notice`).
pub const NOTICE: Id = Id::new_static("menu.notice");
/// How menus look: a list ([`STYLE_LIST`]) or tiles ([`STYLE_TILES`]).
pub const STYLE: Id = Id::new_static("menu.style");
pub const STYLE_LIST: &str = "menu.style.list";
pub const STYLE_TILES: &str = "menu.style.tiles";

mod bar;
mod notice;
mod panel;

/// The menu plugin.
#[derive(Debug, Default)]
pub struct Menu;

impl Plugin for Menu {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Menu"),
            description: Cow::Borrowed(
                "The menu at the bottom of the screen, with actions and settings.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("menu.title"),
            factory: Box::new(|env, params| {
                if params.get(params::MODE) == Some(bar::MODE) || tiles(env, params) {
                    Box::new(bar::BarScreen::new(env, params))
                } else {
                    Box::new(MenuScreen::new(env, params))
                }
            }),
        })?;
        r.bind_slot(slots::MENU, SCREEN, Priority::DEFAULT)?;
        r.add_screen(ScreenSpec {
            id: PANEL,
            title: Cow::Borrowed("menu.title"),
            factory: Box::new(|env, params| Box::new(panel::PanelScreen::open(env, params))),
        })?;
        r.bind_slot(slots::PANEL, PANEL, Priority::DEFAULT)?;
        r.add_screen(ScreenSpec {
            id: NOTICE,
            title: Cow::Borrowed("menu.title"),
            factory: Box::new(|env, params| Box::new(notice::NoticeScreen::new(env, params))),
        })?;
        r.bind_slot(slots::NOTICE, NOTICE, Priority::DEFAULT)?;
        r.add_setting(
            SettingSpec::new(
                STYLE,
                "menu.setting.style",
                SettingKind::Choice(alloc::vec![
                    Cow::Borrowed(STYLE_LIST),
                    Cow::Borrowed(STYLE_TILES),
                ]),
                quireink_sdk::SettingValue::Text(Cow::Borrowed(STYLE_LIST)),
            )
            .group(quireink_sdk::builtin::groups::LOOK)
            .description("menu.setting.style.help"),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// An entry of the menu, or a heading.
#[derive(Debug)]
struct Item {
    /// Action, setting or section.
    id: Id,
    /// Translation key of the label.
    title: String,
    icon: Option<String>,
    heading: bool,
}

#[derive(Debug)]
struct MenuScreen {
    items: Vec<Item>,
    style: UiStyle,
    region: Rect,
    rows: ui::Rows,
    /// Shown when the items need more than one page.
    pager: Option<ui::Pager>,
    /// The focused number is being changed with buttons.
    editing: bool,
    /// The focus is shown: always with buttons only, on touch screens
    /// once buttons are used (a tap needs no focus).
    keys: bool,
    /// Room for icons left of the labels, if any entry has one.
    icon_width: u16,
    /// How many menus are open below this one (submenus).
    depth: usize,
}

/// Parameter: the depth of a submenu.
pub(crate) const DEPTH: &str = "depth";

/// The settings plugin's list for choosing a value.
pub(crate) const CHOOSE_SCREEN: Id = Id::new_static("settings.choose");

impl MenuScreen {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let main = menus::MAIN;
        let menu = params.get(params::MENU).unwrap_or(main.as_str());
        let mut items: Vec<Item> = env
            .menus
            .items(menu)
            .into_iter()
            .map(|item| Item {
                id: item.id.clone(),
                title: String::from(item.title),
                icon: item.icon.map(String::from),
                heading: item.section,
            })
            .collect();
        // Headings with nothing below them are left out.
        let keep: Vec<bool> = (0..items.len())
            .map(|i| !items[i].heading || items.get(i + 1).is_some_and(|next| !next.heading))
            .collect();
        let mut keep = keep.into_iter();
        items.retain(|_| keep.next().unwrap_or(true));
        let style = UiStyle::new(env);
        let display = env.device.display;
        let border = style.line * 2;
        let heading = ui::heading_height(env, &style);
        let heights: Vec<u16> = items
            .iter()
            .map(|item| {
                if item.heading {
                    heading
                } else {
                    style.row as u16
                }
            })
            .collect();
        let total: u32 = heights
            .iter()
            .map(|&h| u32::from(h))
            .sum::<u32>()
            .max(style.row);
        let wanted = total + 2 * style.gap + border;
        // Up to the header of the screen below, which stays in sight.
        let most = u32::from(display.height).saturating_sub(style.header);
        let height = wanted.min(most) as u16;
        let region = Rect::new(0, display.height - height, display.width, height);
        let mut list_area = Rect::new(
            0,
            region.y + (border + style.gap) as u16,
            display.width,
            height.saturating_sub((border + 2 * style.gap) as u16),
        );
        // Too many items: a bar at the bottom shows that there are more
        // pages and turns them.
        let mut pager = None;
        if wanted > u32::from(height) {
            let bar = ui::Pager::height(&style);
            list_area.height = list_area.height.saturating_sub(bar);
            let area = Rect::new(0, region.y + region.height - bar, display.width, bar);
            pager = Some(ui::Pager::new(env, area));
        }
        let icon_width = ui::icon_room(
            env,
            &style,
            items.iter().any(|i| i.icon.is_some() && !i.heading),
        );
        let headings = items.iter().map(|i| i.heading).collect();
        Self {
            items,
            region,
            rows: ui::Rows::new(list_area, heights, headings),
            pager,
            editing: false,
            keys: !env.device.input.touch,
            icon_width,
            depth: params.get(DEPTH).and_then(|d| d.parse().ok()).unwrap_or(0),
            style,
        }
    }

    /// The setting behind item `index`, if it is one.
    fn setting<'e>(env: &Env<'e>, id: &Id) -> Option<&'e SettingSpec> {
        env.settings.specs().find(|spec| spec.id == *id)
    }

    /// Changes a setting by one step.
    fn step(&self, env: &Env<'_>, spec: &SettingSpec, step: i64, cx: &mut Cx<'_, '_>) {
        if let Some(value) = ui::next_value(env, spec, step)
            && Some(&value) != env.settings.get(spec.id.as_str())
        {
            cx.command(Command::SetSetting(spec.id.clone(), value));
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn moved(change: ListChange, cx: &mut Cx<'_, '_>) {
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    /// Chooses item `index`: runs an action, or changes a setting.
    fn run(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(item) = self.items.get(index).filter(|i| !i.heading) else {
            return;
        };
        let id = &item.id;
        if is_menu(&env, id) {
            cx.command(Command::OpenScreen(
                SCREEN,
                Params::new()
                    .with(params::MENU, id.as_str().to_string())
                    .with(DEPTH, alloc::format!("{}", self.depth + 1)),
            ));
            return;
        }
        let Some(spec) = Self::setting(&env, id) else {
            // Closes this menu and those below it, so the action reaches
            // the screen the menu was opened on.
            for _ in 0..=self.depth {
                cx.command(Command::Back);
            }
            cx.command(Command::RunAction(id.clone()));
            return;
        };
        if let Some(editor) = &spec.editor {
            cx.command(Command::OpenScreen(
                editor.clone(),
                Params::new().with("id", spec.id.as_str().to_string()),
            ));
            return;
        }
        match spec.kind {
            SettingKind::Int { .. } => {
                self.editing = !self.editing;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
            // The list of the standard settings plugin.
            SettingKind::Choice(_) | SettingKind::Language => {
                cx.command(Command::OpenScreen(
                    CHOOSE_SCREEN,
                    Params::new().with("id", spec.id.as_str().to_string()),
                ));
            }
            _ => self.step(&env, spec, 1, cx),
        }
    }

    /// Draws entry `item` in `rect`: its icon, label and value.
    fn draw_entry(
        &self,
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        item: &Item,
        rect: Rect,
        focused: bool,
    ) {
        let style = self.style;
        let Some(font) = style.font(env) else {
            return;
        };
        let ink = if focused { Color::WHITE } else { Color::BLACK };
        // The label and value right of the icons.
        let rect = ui::draw_row_icon(
            canvas,
            env,
            &style,
            rect,
            self.icon_width,
            item.icon.as_deref(),
            ink,
        );
        let label = env.i18n.tr(&item.title);
        match Self::setting(env, &item.id) {
            Some(spec) if matches!(spec.kind, SettingKind::Bool) => {
                let on = env.settings.bool(spec.id.as_str());
                ui::draw_switch_row(canvas, env, &style, rect, focused, label, "", on);
            }
            Some(spec) if focused && matches!(spec.kind, SettingKind::Int { .. }) => {
                let value = ui::setting_value(env, spec);
                ui::draw_stepper(canvas, env, &style, rect, label, &value, self.editing);
            }
            Some(spec) => {
                let mut value = ui::setting_value(env, spec);
                if spec.editor.is_some()
                    || matches!(spec.kind, SettingKind::Choice(_) | SettingKind::Language)
                {
                    value.push_str(" ›");
                }
                ui::draw_value_row(canvas, env, &style, rect, focused, label, &value);
            }
            None if is_menu(env, &item.id) => {
                ui::draw_value_row(canvas, env, &style, rect, focused, label, "›");
            }
            None => {
                let x = f32::from(rect.x) + style.margin as f32;
                let width = f32::from(rect.width) - 2.0 * style.margin as f32;
                let label = ui::fit(&font, style.text, label, width);
                ui::draw_label(canvas, &font, style.text, x, rect, &label, ink);
            }
        }
    }
}

/// Whether the menu in `params` opens as tiles: the user wants them, it
/// is a menu of its own (not a submenu) and every entry has an icon.
fn tiles(env: &Env<'_>, params: &Params) -> bool {
    if env.settings.text(STYLE.as_str()) != STYLE_TILES || params.get(DEPTH).is_some() {
        return false;
    }
    let items = env
        .menus
        .items(params.get(params::MENU).unwrap_or_default());
    let entries: Vec<_> = items.iter().filter(|i| !i.section).collect();
    !entries.is_empty() && entries.iter().all(|i| i.icon.is_some())
}

/// Whether `id` names a menu with entries (a submenu).
pub(crate) fn is_menu(env: &Env<'_>, id: &Id) -> bool {
    !env.menus.items(id.as_str()).is_empty()
}

impl Screen for MenuScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        ui::fill(canvas, self.region, Color::WHITE);
        let border = Rect::new(
            self.region.x,
            self.region.y,
            self.region.width,
            (style.line * 2) as u16,
        );
        ui::fill(canvas, border, Color::BLACK);
        if style.font(env).is_none() {
            return;
        }
        if self.items.is_empty() {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.rows.area,
                env.i18n.tr("menu.empty"),
            );
            return;
        }
        let items = &self.items;
        self.rows
            .draw(canvas, &style, self.keys, |canvas, index, rect, focused| {
                let item = &items[index];
                if item.heading {
                    ui::draw_heading(canvas, env, &style, rect, env.i18n.tr(&item.title));
                } else {
                    self.draw_entry(canvas, env, item, rect, focused);
                }
            });
        if let Some(pager) = &self.pager {
            pager.draw(canvas, env, &style, self.rows.paging());
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if self.editing {
            let env = *cx.env();
            let spec = self
                .items
                .get(self.rows.focus)
                .and_then(|item| Self::setting(&env, &item.id));
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
            match spec {
                Some(spec) if decrease.contains(a) => self.step(&env, spec, -1, cx),
                Some(spec) if increase.contains(a) => self.step(&env, spec, 1, cx),
                _ if *a == actions::CONFIRM || *a == actions::BACK => {
                    self.editing = false;
                    cx.request_redraw();
                    cx.request_refresh(RefreshMode::Fast);
                }
                _ => return Handled::No,
            }
            return Handled::Yes;
        }
        let navigation = [
            actions::UP,
            actions::LEFT,
            actions::PREVIOUS,
            actions::DOWN,
            actions::RIGHT,
            actions::NEXT,
            actions::NEXT_PAGE,
            actions::PREV_PAGE,
        ];
        if navigation.contains(a) && !self.keys {
            // The first button press shows where the focus is.
            self.keys = true;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return Handled::Yes;
        }
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            Self::moved(self.rows.step(false), cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            Self::moved(self.rows.step(true), cx);
        } else if *a == actions::NEXT_PAGE {
            Self::moved(self.rows.turn(true), cx);
        } else if *a == actions::PREV_PAGE {
            Self::moved(self.rows.turn(false), cx);
        } else if *a == actions::CONFIRM {
            self.run(self.rows.focus, cx);
        } else if *a == actions::MENU {
            // The menu button toggles the menu.
            cx.command(Command::Back);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(action) = self.pager.and_then(|pager| pager.tap(gesture.start)) {
            let forward = action == actions::NEXT_PAGE;
            Self::moved(self.rows.turn(forward), cx);
            return Handled::Yes;
        }
        let Some(index) = self.rows.hit(gesture.start) else {
            // Taps between the items do nothing; outside, they close it.
            return Handled::Yes;
        };
        let env = *cx.env();
        let number = self.items.get(index).and_then(|item| {
            Self::setting(&env, &item.id).filter(|s| matches!(s.kind, SettingKind::Int { .. }))
        });
        let Some(spec) = number else {
            self.rows.focus = index;
            self.editing = false;
            self.run(index, cx);
            return Handled::Yes;
        };
        // The first tap selects a number; its − and + buttons change it.
        if index != self.rows.focus || !self.keys {
            self.rows.focus = index;
            self.keys = true;
            self.editing = false;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return Handled::Yes;
        }
        let Some(rect) = self.rows.rect_of(index) else {
            return Handled::Yes;
        };
        let rect = Rect::new(
            rect.x + self.icon_width,
            rect.y,
            rect.width.saturating_sub(self.icon_width),
            rect.height,
        );
        let value = ui::setting_value(&env, spec);
        if let Some((minus, plus)) = ui::stepper(&env, &self.style, rect, &value) {
            let x = gesture.start.x;
            let within = |r: Rect| x >= i32::from(r.x) && x < r.right() as i32;
            if within(minus) {
                self.step(&env, spec, -1, cx);
            } else if within(plus) {
                self.step(&env, spec, 1, cx);
            }
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        // Values shown in the menu may have changed.
        if *event == events::SETTING_CHANGED {
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
