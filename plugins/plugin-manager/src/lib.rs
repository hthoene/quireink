//! The plugin manager: lists every plugin with its name, version and
//! description, and switches plugins on and off.
//!
//! - Each row shows a plugin and its state: *On*, *Off*, *Always on* (the
//!   kernel), *Required* (it alone provides e.g. the settings or the menu,
//!   so switching it off would leave no way back; once another plugin
//!   provides them too, it can be switched off) or *Error*.
//! - Confirming or tapping a row switches the plugin. The choice is saved
//!   to `quireink.ini` (`[plugins] id = off`) and applies after a restart:
//!   the bar at the bottom then offers *Restart now*. The bar is always
//!   there (with a hint before), so nothing moves when it changes.
//! - Below each name: what happens to the plugin (if anything), its
//!   description and version, readable without tapping it.
//!
//! The manager is itself a plugin and reached from the main settings page
//! (`core.settings-pages`); another plugin can replace it.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{CORE_PLUGIN, actions, menus};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, PluginInfo, PluginState, Rect, RefreshMode, RegisterError,
    Registrar, SDK_VERSION, Screen, ScreenSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("plugin-manager");
/// The list of plugins.
pub const SCREEN: Id = Id::new_static("plugin-manager.list");
/// Opens [`SCREEN`].
pub const OPEN: Id = Id::new_static("plugin-manager.open");

/// The plugin manager plugin.
#[derive(Debug, Default)]
pub struct PluginManager;

impl Plugin for PluginManager {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Plugin manager"),
            description: Cow::Borrowed("Lists the plugins and switches them on and off."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("plugin-manager.title"),
            factory: Box::new(|env, _| Box::new(PluginList::new(env))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("plugin-manager.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::SETTINGS,
            item: OPEN,
            order: 220,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("plug"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Where the focus is, for buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    /// The *Restart now* button of the bar.
    Restart,
}

#[derive(Debug)]
struct PluginList {
    /// Plugin ids: the kernel first, then by name.
    plugins: Vec<Id>,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// The bar at the bottom: a hint, or *Restart now*.
    bar: Rect,
    focus: Focus,
}

impl PluginList {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let bar_height = style.row as u16 + style.gap as u16;
        let bar = Rect::new(0, display.height - bar_height, display.width, bar_height);
        let height = display.height.saturating_sub(top + bar_height);
        let area = Rect::new(0, top, display.width, height);
        let list = ListView::new(area, style.noted_row());
        let mut plugins: Vec<&PluginInfo> = env.plugins.iter().collect();
        plugins.sort_by_key(|p| {
            (
                p.manifest.id != CORE_PLUGIN,
                p.name(env.i18n).to_lowercase(),
            )
        });
        Self {
            plugins: plugins.iter().map(|p| p.manifest.id.clone()).collect(),
            style,
            header,
            list,
            bar,
            focus: Focus::List,
        }
    }

    /// The *Restart now* button, while a change waits for a restart.
    fn restart_button(&self, env: &Env<'_>) -> Option<Rect> {
        if !env.plugins.restart_needed() {
            return None;
        }
        let font = self.style.font(env)?;
        let label = env.i18n.tr("plugin-manager.restart");
        let pad = self.style.margin as u16;
        let width = font.measure(self.style.text, label) as u16 + 2 * pad;
        let height = ui::button_height(self.bar);
        Some(Rect::new(
            self.bar.width.saturating_sub(width + pad),
            self.bar.y + (self.bar.height - height) / 2,
            width,
            height,
        ))
    }

    /// Switches the plugin `id`.
    fn switch(&mut self, env: &Env<'_>, id: &Id, cx: &mut Cx<'_, '_>) {
        let Some(info) = env.plugins.get(id.as_str()) else {
            return;
        };
        if info.enabled && locked(info) {
            // The description below says why.
            return;
        }
        cx.command(Command::SetPluginEnabled(id.clone(), !info.enabled));
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

/// Whether the plugin must stay on: the kernel says so, or it is this
/// manager (without it, plugins could not be switched on again).
fn locked(info: &PluginInfo) -> bool {
    !info.can_switch_off() || info.manifest.id == PLUGIN
}

/// The state shown on the right of a row.
fn state(info: &PluginInfo) -> &'static str {
    if info.manifest.id == CORE_PLUGIN {
        "plugin-manager.always-on"
    } else if info.state == PluginState::Failed && info.enabled {
        "plugin-manager.error"
    } else if locked(info) {
        "plugin-manager.required"
    } else if info.enabled {
        "plugin-manager.on"
    } else {
        "plugin-manager.off"
    }
}

/// The note below a plugin's name: what happens to it, its description
/// and version.
fn details(env: &Env<'_>, info: &PluginInfo) -> String {
    let i18n = env.i18n;
    let version = i18n.format(
        "plugin-manager.version",
        &[("version", Arg::Str(&info.manifest.version.to_string()))],
    );
    let note = if info.manifest.id == CORE_PLUGIN {
        None
    } else if info.manifest.id == PLUGIN && info.required_for.is_empty() {
        Some(i18n.tr("plugin-manager.self").to_string())
    } else if !info.required_for.is_empty() {
        let what: Vec<&str> = info
            .required_for
            .iter()
            .map(|slot| i18n.tr(slot_key(slot)))
            .collect();
        Some(i18n.format(
            "plugin-manager.needed-for",
            &[("what", Arg::Str(&what.join(", ")))],
        ))
    } else if info.restart_needed() {
        Some(
            i18n.tr(if info.enabled {
                "plugin-manager.starts-after-restart"
            } else {
                "plugin-manager.stops-after-restart"
            })
            .to_string(),
        )
    } else if info.state == PluginState::Failed {
        Some(i18n.tr("plugin-manager.failed").to_string())
    } else {
        None
    };
    let description = info.description(i18n);
    match note {
        Some(note) => format!("{note} {description} {version}"),
        None => format!("{description} {version}"),
    }
}

/// Translation key of a standard slot's name.
fn slot_key(slot: &Id) -> &'static str {
    match slot.as_str() {
        "core.home" => "plugin-manager.slot.home",
        "core.reader" => "plugin-manager.slot.reader",
        "core.library" => "plugin-manager.slot.library",
        "core.menu" => "plugin-manager.slot.menu",
        "core.settings" => "plugin-manager.slot.settings",
        "core.font-ui" => "plugin-manager.slot.fonts",
        _ => "plugin-manager.slot.other",
    }
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

impl Screen for PluginList {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.plugins.len();
        self.list.focus = self.list.focus.min(len.saturating_sub(1));
        if self.focus == Focus::Restart && self.restart_button(env).is_none() {
            self.focus = Focus::List;
        }
        self.list.active = self.focus == Focus::List;
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("plugin-manager.title"),
            None,
            self.list.paging(len),
        );
        let plugins = &self.plugins;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                if let Some(info) = env.plugins.get(plugins[index].as_str()) {
                    let value = env.i18n.tr(state(info));
                    let name = info.name(env.i18n);
                    let note = details(env, info);
                    ui::draw_noted_row(canvas, env, &style, rect, focused, name, value, &note);
                }
            });
        // The bar: a line above it, the hint or what a restart does, and
        // the button.
        let line = Rect::new(0, self.bar.y, self.bar.width, (style.line as u16).max(1));
        ui::fill(canvas, line, quireink_sdk::Color::gray(150));
        let button = self.restart_button(env);
        if let Some(button) = button {
            let label = env.i18n.tr("plugin-manager.restart");
            let focused = self.focus == Focus::Restart;
            ui::draw_button(canvas, env, &style, button, label, true, focused);
        }
        if let Some(font) = style.font(env) {
            let key = if button.is_some() {
                "plugin-manager.restart.short"
            } else {
                "plugin-manager.hint"
            };
            let right = button.map_or(self.bar.width, |b| b.x);
            let area = Rect::new(
                0,
                self.bar.y,
                right.saturating_sub(style.gap as u16),
                self.bar.height,
            );
            ui::draw_label(
                canvas,
                &font,
                style.small,
                style.margin as f32,
                area,
                env.i18n.tr(key),
                quireink_sdk::Color::gray(60),
            );
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let len = self.plugins.len();
        let page = self.list.rows() as isize;
        let a = action;
        // Down from the last plugin reaches the bar's button, up comes back.
        let restart = self.restart_button(&env).is_some();
        if self.focus == Focus::Restart {
            if *a == actions::CONFIRM {
                cx.command(Command::Restart);
            } else if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
                self.focus = Focus::List;
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            } else if *a != actions::DOWN && *a != actions::RIGHT && *a != actions::NEXT {
                return Handled::No;
            }
            return Handled::Yes;
        }
        let down = [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a);
        if down && restart && self.list.focus + 1 >= len {
            self.focus = Focus::Restart;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return Handled::Yes;
        }
        let delta = if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            -1
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            1
        } else if *a == actions::NEXT_PAGE {
            page
        } else if *a == actions::PREV_PAGE {
            -page
        } else if *a == actions::CONFIRM {
            if let Some(id) = self.plugins.get(self.list.focus).cloned() {
                self.switch(&env, &id, cx);
            }
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        moved(self.list.move_focus(delta, len), cx);
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = self.list.paging(self.plugins.len());
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let env = *cx.env();
        if self
            .restart_button(&env)
            .is_some_and(|b| b.contains(gesture.start))
        {
            cx.command(Command::Restart);
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, self.plugins.len()) else {
            return Handled::No;
        };
        self.list.focus = index;
        self.focus = Focus::List;
        let id = self.plugins[index].clone();
        self.switch(&env, &id, cx);
        Handled::Yes
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    use super::*;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn standard_slots_have_names() {
        for slot in [
            "core.home",
            "core.reader",
            "core.library",
            "core.menu",
            "core.settings",
            "core.font-ui",
        ] {
            assert_ne!(slot_key(&Id::new_static(slot)), "plugin-manager.slot.other");
        }
    }
}
