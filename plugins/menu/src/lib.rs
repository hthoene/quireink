//! The menu overlay.
//!
//! Shows the menu named by the `menu` parameter at the bottom of the screen,
//! above whatever is open. Entries come from every plugin (see
//! `Registrar::add_menu_entry`) or from the user's `[menus]` definitions.
//! Choosing an action closes the menu and runs it. Settings show their
//! current value and change in place, so the menu stays open: switches
//! toggle, numbers have − and + buttons, choices open a list.
//!
//! Bound to the `core.menu` slot — replace it to get a different menu style
//! (radial, side bar, …) for every menu at once.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, menus, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params, Plugin,
    Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("menu");
pub const SCREEN: Id = Id::new_static("menu.list");

/// Most of the screen a menu may cover.
const MAX_SHARE: f32 = 0.75;

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
            factory: Box::new(|env, params| Box::new(MenuScreen::new(env, params))),
        })?;
        r.bind_slot(slots::MENU, SCREEN, Priority::DEFAULT)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[derive(Debug)]
struct MenuScreen {
    /// `(action or setting, translation key of the label)`.
    items: Vec<(Id, String)>,
    style: UiStyle,
    region: Rect,
    list: ListView,
    /// The focused number is being changed with buttons.
    editing: bool,
}

/// The settings plugin's list for choosing a value.
const CHOOSE_SCREEN: Id = Id::new_static("settings.choose");

impl MenuScreen {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let main = menus::MAIN;
        let menu = params.get(params::MENU).unwrap_or(main.as_str());
        let items: Vec<(Id, String)> = env
            .menus
            .items(menu)
            .into_iter()
            .map(|item| (item.id.clone(), String::from(item.title)))
            .collect();
        let style = UiStyle::new(env);
        let display = env.device.display;
        let border = style.line * 2;
        let wanted = items.len().max(1) as u32 * style.row + 2 * style.gap + border;
        let height = wanted.min((f32::from(display.height) * MAX_SHARE) as u32) as u16;
        let region = Rect::new(0, display.height - height, display.width, height);
        let list_area = Rect::new(
            0,
            region.y + (border + style.gap) as u16,
            display.width,
            height.saturating_sub((border + 2 * style.gap) as u16),
        );
        Self {
            items,
            region,
            list: ListView::new(list_area, style.row),
            style,
            editing: false,
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
        let Some((id, _)) = self.items.get(index) else {
            return;
        };
        let Some(spec) = Self::setting(&env, id) else {
            cx.command(Command::Back);
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
        let Some(font) = style.font(env) else {
            return;
        };
        if self.items.is_empty() {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("menu.empty"),
            );
            return;
        }
        let items = &self.items;
        let editing = self.editing;
        self.list.draw(
            canvas,
            &style,
            items.len(),
            |canvas, index, rect, focused| {
                let (id, title) = &items[index];
                let label = env.i18n.tr(title);
                match Self::setting(env, id) {
                    Some(spec) if focused && matches!(spec.kind, SettingKind::Int { .. }) => {
                        let value = ui::setting_value(env, spec);
                        ui::draw_stepper(canvas, env, &style, rect, label, &value, editing);
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
                    None => {
                        let color = if focused { Color::WHITE } else { Color::BLACK };
                        let x = style.margin as f32;
                        ui::draw_label(canvas, &font, style.text, x, rect, label, color);
                    }
                }
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.items.len();
        let rows = self.list.rows() as isize;
        let a = action;
        if self.editing {
            let env = *cx.env();
            let spec = self
                .items
                .get(self.list.focus)
                .and_then(|(id, _)| Self::setting(&env, id));
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
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            Self::moved(self.list.move_focus(-1, len), cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            Self::moved(self.list.move_focus(1, len), cx);
        } else if *a == actions::NEXT_PAGE {
            Self::moved(self.list.move_focus(rows, len), cx);
        } else if *a == actions::PREV_PAGE {
            Self::moved(self.list.move_focus(-rows, len), cx);
        } else if *a == actions::CONFIRM {
            self.run(self.list.focus, cx);
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
        let Some(index) = self.list.hit(gesture.start, self.items.len()) else {
            return Handled::No;
        };
        let env = *cx.env();
        let number = self.items.get(index).and_then(|(id, _)| {
            Self::setting(&env, id).filter(|s| matches!(s.kind, SettingKind::Int { .. }))
        });
        let Some(spec) = number else {
            self.list.focus = index;
            self.editing = false;
            self.run(index, cx);
            return Handled::Yes;
        };
        // The first tap selects a number; its − and + buttons change it.
        if index != self.list.focus {
            self.list.focus = index;
            self.editing = false;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return Handled::Yes;
        }
        let rect = self.list.row_rect(index % self.list.rows());
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
