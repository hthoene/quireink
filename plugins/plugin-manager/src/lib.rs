//! The plugin manager: lists every plugin with its name, version and
//! description, and switches plugins on and off.
//!
//! - Each row shows a plugin and its state: *On*, *Off*, *Always on* (the
//!   kernel), *Required* (it alone provides e.g. the settings or the menu,
//!   so switching it off would leave no way back) or *Error*.
//! - Confirming or tapping a row switches the plugin. The choice is saved
//!   to `quireink.ini` (`[plugins] id = off`) and applies after a restart;
//!   a *Restart now* row appears at the top until then.
//! - Below the list: the focused plugin's description, version and what
//!   happens to it.
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

/// Lines of text below the list.
const FOOTER_LINES: u32 = 4;

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
            order: 100,
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Restart,
    /// A plugin, by id.
    Plugin(Id),
}

#[derive(Debug)]
struct PluginList {
    /// Plugin ids: the kernel first, then by name.
    plugins: Vec<Id>,
    style: UiStyle,
    header: Rect,
    list: ListView,
}

impl PluginList {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = Rect::new(0, 0, display.width, style.header as u16);
        let top = header.height + style.gap as u16;
        let height = display
            .height
            .saturating_sub(top)
            .saturating_sub(ui::footer_height(&style, FOOTER_LINES));
        let list = ListView::new(Rect::new(0, top, display.width, height), style.row);
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
        }
    }

    /// The rows: *Restart now* while a change is pending, then the plugins.
    fn rows(&self, env: &Env<'_>) -> Vec<Row> {
        let restart = env.plugins.restart_needed().then_some(Row::Restart);
        restart
            .into_iter()
            .chain(self.plugins.iter().cloned().map(Row::Plugin))
            .collect()
    }

    fn info<'e>(env: &Env<'e>, row: &Row) -> Option<&'e PluginInfo> {
        match row {
            Row::Plugin(id) => env.plugins.get(id.as_str()),
            Row::Restart => None,
        }
    }

    /// Switches the plugin in `row`, or restarts.
    fn activate(&mut self, env: &Env<'_>, row: &Row, cx: &mut Cx<'_, '_>) {
        match row {
            Row::Restart => cx.command(Command::Restart),
            Row::Plugin(id) => {
                let Some(info) = env.plugins.get(id.as_str()) else {
                    return;
                };
                if info.enabled && locked(info) {
                    // The description below says why.
                    return;
                }
                let had_restart_row = env.plugins.restart_needed();
                cx.command(Command::SetPluginEnabled(id.clone(), !info.enabled));
                // Keep the focus on the plugin when the restart row comes
                // or goes.
                let pending = env
                    .plugins
                    .iter()
                    .any(|p| p.manifest.id != *id && p.restart_needed())
                    || !info.restart_needed();
                match (had_restart_row, pending) {
                    (false, true) => self.list.focus += 1,
                    (true, false) => self.list.focus = self.list.focus.saturating_sub(1),
                    _ => {}
                }
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
        }
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

/// The text below the list for a plugin: description, version and what
/// happens to it.
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
    let mut text = format!("{} {version}", info.description(i18n));
    if let Some(note) = note {
        text.push(' ');
        text.push_str(&note);
    }
    text
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
        // The footer follows the focus.
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
        let rows = self.rows(env);
        let len = rows.len();
        self.list.focus = self.list.focus.min(len.saturating_sub(1));
        let note = (self.list.pages(len) > 1).then(|| {
            env.i18n.format(
                "plugin-manager.page",
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
            env.i18n.tr("plugin-manager.title"),
            note.as_deref(),
        );
        self.list.draw(
            canvas,
            &style,
            len,
            |canvas, index, rect, focused| match Self::info(env, &rows[index]) {
                Some(info) => {
                    let value = env.i18n.tr(state(info));
                    let name = info.name(env.i18n);
                    ui::draw_value_row(canvas, env, &style, rect, focused, name, value);
                }
                None => {
                    let title = env.i18n.tr("plugin-manager.restart");
                    ui::draw_value_row(canvas, env, &style, rect, focused, title, "›");
                }
            },
        );
        let text = match rows.get(self.list.focus) {
            Some(row) => match Self::info(env, row) {
                Some(info) => details(env, info),
                None => env.i18n.tr("plugin-manager.restart.help").to_string(),
            },
            None => return,
        };
        let top = f32::from(self.list.area.y) + f32::from(self.list.area.height);
        ui::draw_footer(canvas, env, &style, top, FOOTER_LINES, &text);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let rows = self.rows(&env);
        let len = rows.len();
        let page = self.list.rows() as isize;
        let a = action;
        let delta = if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            -1
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            1
        } else if *a == actions::NEXT_PAGE {
            page
        } else if *a == actions::PREV_PAGE {
            -page
        } else if *a == actions::CONFIRM {
            if let Some(row) = rows.get(self.list.focus) {
                self.activate(&env, row, cx);
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
        let env = *cx.env();
        let rows = self.rows(&env);
        let Some(index) = self.list.hit(gesture.start, rows.len()) else {
            return Handled::No;
        };
        if index != self.list.focus {
            self.list.focus = index;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        self.activate(&env, &rows[index], cx);
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
