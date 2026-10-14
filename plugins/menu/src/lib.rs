//! The menu overlay.
//!
//! Shows the menu named by the `menu` parameter at the bottom of the screen,
//! above whatever is open. Entries come from every plugin (see
//! `Registrar::add_menu_entry`) or from the user's `[menus]` definitions;
//! choosing one closes the menu and runs the entry's action.
//!
//! Bound to the `core.menu` slot — replace it to get a different menu style
//! (radial, side bar, …) for every menu at once.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params, Plugin,
    Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    Version,
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
    /// `(action, translation key of the label)`.
    items: Vec<(Id, String)>,
    style: UiStyle,
    region: Rect,
    list: ListView,
}

impl MenuScreen {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let main = menus::MAIN;
        let menu = params.get(params::MENU).unwrap_or(main.as_str());
        let items: Vec<(Id, String)> = env
            .menus
            .items(menu)
            .into_iter()
            .map(|item| (item.action.clone(), String::from(item.title)))
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
        }
    }

    fn moved(change: ListChange, cx: &mut Cx<'_, '_>) {
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn run(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some((action, _)) = self.items.get(index) {
            cx.command(Command::Back);
            cx.command(Command::RunAction(action.clone()));
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
        self.list.draw(
            canvas,
            &style,
            items.len(),
            |canvas, index, rect, focused| {
                let color = if focused { Color::WHITE } else { Color::BLACK };
                let label = env.i18n.tr(&items[index].1);
                ui::draw_label(
                    canvas,
                    &font,
                    style.text,
                    style.margin as f32 * 2.0,
                    rect,
                    label,
                    color,
                );
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.items.len();
        let rows = self.list.rows() as isize;
        let a = action;
        if *a == actions::UP || *a == actions::LEFT {
            Self::moved(self.list.move_focus(-1, len), cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT {
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
        match self.list.hit(gesture.start, self.items.len()) {
            Some(index) => {
                self.list.focus = index;
                self.run(index, cx);
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
