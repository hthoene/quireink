//! The settings screen.
//!
//! Lists every setting any plugin declared (`Registrar::add_setting`) and
//! edits it according to its kind — no plugin needs its own settings UI.
//! Changes are applied immediately and saved to `quireink.ini` by the
//! kernel.
//!
//! Buttons: move to a setting, *confirm* toggles a switch or starts editing
//! a value; while editing, left/right (or up/down, page buttons) change it
//! and *confirm*/*back* finish. Touch: tap a switch to toggle it; tap the
//! left third of a value to decrease it, elsewhere to increase it.
//!
//! Bound to the `core.settings` slot.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params,
    Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("settings");
pub const SCREEN: Id = Id::new_static("settings.list");

/// The settings plugin.
#[derive(Debug, Default)]
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Settings"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("settings.title"),
            factory: Box::new(|env, _| Box::new(SettingsScreen::new(env))),
        })?;
        r.bind_slot(slots::SETTINGS, SCREEN, Priority::DEFAULT)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[derive(Debug)]
struct SettingsScreen {
    specs: Vec<SettingSpec>,
    style: UiStyle,
    header: Rect,
    list: ListView,
    /// The focused value is being edited.
    editing: bool,
}

impl SettingsScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = Rect::new(0, 0, display.width, style.header as u16);
        let top = header.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            specs: env.settings.specs().cloned().collect(),
            list: ListView::new(area, style.row),
            style,
            header,
            editing: false,
        }
    }

    fn focused(&self) -> Option<&SettingSpec> {
        self.specs.get(self.list.focus)
    }

    /// Changes the focused setting by `step` (toggle, ±1, next option).
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
            (SettingKind::Choice(options), Some(SettingValue::Text(v))) => {
                let options: Vec<&str> = options.iter().map(|o| o.as_ref()).collect();
                SettingValue::Text(Cow::Owned(cycle(&options, v, step).to_string()))
            }
            (SettingKind::Language, Some(SettingValue::Text(v))) => {
                let tags: Vec<&str> = env.i18n.languages().into_iter().map(|(t, _)| t).collect();
                SettingValue::Text(Cow::Owned(cycle(&tags, v, step).to_string()))
            }
            _ => return,
        };
        if Some(&value) != current {
            cx.command(Command::SetSetting(spec.id.clone(), value));
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Partial);
        }
    }

    fn editable(spec: &SettingSpec) -> bool {
        !matches!(spec.kind, SettingKind::Text)
    }

    fn moved(&mut self, change: ListChange, cx: &mut Cx<'_, '_>) {
        if change != ListChange::None {
            self.editing = false;
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
    }
}

/// The option `step` positions after `current`, wrapping around.
fn cycle<'a>(options: &[&'a str], current: &str, step: i64) -> &'a str {
    if options.is_empty() {
        return "";
    }
    let len = options.len() as i64;
    let index = options.iter().position(|o| *o == current).unwrap_or(0) as i64;
    options[(index + step).rem_euclid(len) as usize]
}

/// The value of a setting for display.
fn display_value(env: &Env<'_>, spec: &SettingSpec) -> String {
    match env.settings.get(spec.id.as_str()) {
        Some(SettingValue::Bool(true)) => env.i18n.tr("settings.on").to_string(),
        Some(SettingValue::Bool(false)) => env.i18n.tr("settings.off").to_string(),
        Some(SettingValue::Int(v)) => format!("{v}"),
        Some(SettingValue::Text(v)) if spec.kind == SettingKind::Language => env
            .i18n
            .languages()
            .into_iter()
            .find(|(tag, _)| *tag == v.as_ref())
            .map_or(v.to_string(), |(_, name)| name.to_string()),
        // Choices are ids; their translation is their display name.
        Some(SettingValue::Text(v)) if matches!(spec.kind, SettingKind::Choice(_)) => {
            env.i18n.tr(v).to_string()
        }
        Some(SettingValue::Text(v)) => v.to_string(),
        None => String::new(),
    }
}

impl Screen for SettingsScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.specs.len();
        let note = (self.list.pages(len) > 1).then(|| {
            env.i18n.format(
                "settings.page",
                &[
                    ("page", Arg::Int(self.list.page() as i64 + 1)),
                    ("pages", Arg::Int(self.list.pages(len) as i64)),
                ],
            )
        });
        ui::draw_header(
            canvas,
            env,
            &style,
            self.header,
            env.i18n.tr("settings.title"),
            note.as_deref(),
        );
        let Some(font) = style.font(env) else {
            return;
        };
        let specs = &self.specs;
        let editing = self.editing;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let spec = &specs[index];
                let color = if focused { Color::WHITE } else { Color::BLACK };
                let mut value = display_value(env, spec);
                if focused && editing {
                    value = format!("‹ {value} ›");
                }
                let margin = style.margin as f32;
                let value_width = font.measure(style.text, &value);
                let value_x = f32::from(rect.x) + f32::from(rect.width) - margin - value_width;
                ui::draw_label(canvas, &font, style.text, value_x, rect, &value, color);
                let title = ui::fit(
                    &font,
                    style.text,
                    env.i18n.tr(&spec.title),
                    value_x - margin * 3.0,
                );
                ui::draw_label(canvas, &font, style.text, margin, rect, &title, color);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let len = self.specs.len();
        let a = action;
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
            if decrease.contains(a) {
                self.change(&env, -1, cx);
            } else if increase.contains(a) {
                self.change(&env, 1, cx);
            } else if *a == actions::CONFIRM || *a == actions::BACK {
                self.editing = false;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            } else {
                return Handled::No;
            }
            return Handled::Yes;
        }
        let rows = self.list.rows() as isize;
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            let change = self.list.move_focus(-1, len);
            self.moved(change, cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            let change = self.list.move_focus(1, len);
            self.moved(change, cx);
        } else if *a == actions::NEXT_PAGE {
            let change = self.list.move_focus(rows, len);
            self.moved(change, cx);
        } else if *a == actions::PREV_PAGE {
            let change = self.list.move_focus(-rows, len);
            self.moved(change, cx);
        } else if *a == actions::CONFIRM {
            match self.focused().map(|s| &s.kind) {
                Some(SettingKind::Bool) => self.change(&env, 1, cx),
                Some(_) if self.focused().is_some_and(Self::editable) => {
                    self.editing = true;
                    cx.request_redraw();
                    cx.request_refresh(RefreshMode::Fast);
                }
                _ => {}
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
        let Some(index) = self.list.hit(gesture.start, self.specs.len()) else {
            return Handled::No;
        };
        let env = *cx.env();
        if index != self.list.focus {
            self.list.focus = index;
            self.editing = false;
            cx.request_redraw();
        }
        let width = i32::from(self.list.area.width);
        let step = if gesture.start.x < width / 3 { -1 } else { 1 };
        self.change(&env, step, cx);
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED {
            // Values or the UI size may have changed elsewhere.
            let env = *cx.env();
            let focus = self.list.focus;
            let editing = self.editing;
            *self = Self::new(&env);
            self.list.focus = focus.min(self.specs.len().saturating_sub(1));
            self.editing = editing;
            cx.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn cycles_through_options() {
        let options = ["de", "en", "fr"];
        assert_eq!(cycle(&options, "en", 1), "fr");
        assert_eq!(cycle(&options, "fr", 1), "de");
        assert_eq!(cycle(&options, "de", -1), "fr");
        assert_eq!(cycle(&options, "xx", 1), "en");
        assert_eq!(cycle(&[], "en", 1), "");
    }
}
