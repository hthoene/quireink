//! Reading profiles: the settings for reading, the display and the status
//! lines saved under a name ("Night", "Commute", "Large print") and
//! applied again with a tap.
//!
//! - *Profiles* (in the reader's menu under *More*, and in Settings ›
//!   Customise) lists them; the one in use is marked. Tapping one applies
//!   it; holding it offers to save the current settings into it, rename
//!   it or remove it. *Save as new profile* asks for a name.
//! - *Next profile* applies the profile after the one applied last: bind
//!   it to a button or use it in a rule.

#![no_std]

extern crate alloc;

mod profile;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus, params, slots};
use quireink_sdk::ui::choice::{self, Choice};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, Version,
};

use crate::profile::Profile;

pub const PLUGIN: Id = Id::new_static("profiles");
/// Opens the list of profiles.
pub const OPEN: Id = Id::new_static("profiles.open");
/// Applies the profile after the one applied last.
pub const NEXT: Id = Id::new_static("profiles.next");
const LIST_SCREEN: Id = Id::new_static("profiles.list");
const CHOICE: Id = Id::new_static("profiles.choice");
/// Replies: a name for a new profile, a new name, an option.
const NAMED: Id = Id::new_static("profiles.named");
const RENAMED: Id = Id::new_static("profiles.renamed");
const CHOSEN: Id = Id::new_static("profiles.chosen");
/// The reader's menu with more entries.
const READER_MORE: Id = Id::new_static("reader.more");

/// Applies `profile` and says so.
fn apply(env: &Env<'_>, profile: &Profile, out: &mut Vec<Command>) {
    out.extend(profile::apply(env, profile));
    profile::set_last(env.storage, &profile.name);
    out.push(Command::Notify(env.i18n.format(
        "profiles.applied",
        &[("name", Arg::Str(&profile.name))],
    )));
}

/// The profiles plugin.
#[derive(Debug, Default)]
pub struct Profiles;

impl Plugin for Profiles {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Reading profiles"),
            description: Cow::Borrowed(
                "Named sets of reading and display settings, switched with a tap.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: LIST_SCREEN,
            title: Cow::Borrowed("profiles.title"),
            factory: Box::new(|env, _| Box::new(ListScreen::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("profiles.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("profiles.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(LIST_SCREEN, Params::new()));
            })),
        })?;
        r.add_action(ActionSpec {
            id: NEXT,
            title: Cow::Borrowed("profiles.next"),
            handler: Some(Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                let profiles = profile::load(env.storage);
                let last = profile::last(env.storage);
                let at = profiles.iter().position(|p| p.name == last);
                let next = at.map_or(0, |at| (at + 1) % profiles.len().max(1));
                if let Some(profile) = profiles.get(next) {
                    apply(env, profile, out);
                }
            })),
        })?;
        for (menu, order) in [(READER_MORE, 15), (menus::SETTINGS, 205)] {
            r.add_menu_entry(MenuEntry {
                menu,
                item: OPEN,
                order,
            })?;
        }
        r.add_icon(OPEN, Cow::Borrowed("user"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Profile(usize),
    Add,
}

struct ListScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// The line at the bottom.
    status: Rect,
    profiles: Vec<Profile>,
}

impl ListScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let line = (style.row as u16) * 3 / 2;
        let status = Rect::new(0, display.height - line, display.width, line);
        let area = Rect::new(
            0,
            top,
            display.width,
            display.height.saturating_sub(top + line),
        );
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            status,
            profiles: profile::load(env.storage),
            style,
            header,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = (0..self.profiles.len()).map(Row::Profile).collect();
        rows.push(Row::Add);
        rows
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn ask_name(&self, cx: &mut Cx<'_, '_>, reply: Id, text: &str, context: &str) {
        let env = *cx.env();
        cx.command(Command::OpenSlot(
            slots::KEYBOARD,
            Params::new()
                .with(params::TITLE, env.i18n.tr("profiles.name").to_string())
                .with(params::TEXT, text.to_string())
                .with(params::PATH, context.to_string())
                .with(params::REPLY, reply.as_str()),
        ));
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match self.rows().get(index).copied() {
            Some(Row::Profile(i)) => {
                let mut out = Vec::new();
                apply(&env, &self.profiles[i], &mut out);
                for command in out {
                    cx.command(command);
                }
                self.redraw(cx);
            }
            Some(Row::Add) => {
                let name = env.i18n.format(
                    "profiles.default-name",
                    &[("number", Arg::Int(self.profiles.len() as i64 + 1))],
                );
                self.ask_name(cx, NAMED, &name, "");
            }
            None => {}
        }
    }

    fn offer(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some(Row::Profile(i)) = self.rows().get(index).copied() else {
            return self.activate(index, cx);
        };
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &self.profiles[i].name,
            &i.to_string(),
            "profiles.option.",
            &["update", "rename", "remove"],
        ));
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        let len = self.rows().len();
        if self.list.move_focus(delta, len) != ListChange::None {
            self.redraw(cx);
        }
    }

    fn save(&self, env: &Env<'_>) {
        profile::save(env.storage, &self.profiles);
    }
}

/// A profile's name as entered: one line, never empty.
fn clean(name: &str) -> Option<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.replace(['[', ']'], "");
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

impl Screen for ListScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("profiles.title"),
            None,
            self.list.paging(rows.len()),
        );
        let profiles = &self.profiles;
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| match rows[index] {
                Row::Profile(i) => {
                    let p = &profiles[i];
                    let value = if profile::in_use(env, p) {
                        env.i18n.tr("profiles.in-use").to_string()
                    } else {
                        String::new()
                    };
                    let note = env
                        .i18n
                        .plural("profiles.settings", p.values.len() as i64, &[]);
                    ui::draw_noted_row(canvas, env, &style, rect, focused, &p.name, &value, &note);
                }
                Row::Add => {
                    let title = env.i18n.tr("profiles.add");
                    let note = env.i18n.tr("profiles.add.help");
                    ui::draw_noted_row(canvas, env, &style, rect, focused, title, "+", note);
                }
            },
        );
        ui::fill(canvas, self.status, Color::WHITE);
        let Some(font) = style.font(env) else {
            return;
        };
        let line = Rect::new(
            self.status.x + style.margin as u16,
            self.status.y,
            self.status.width.saturating_sub(2 * style.margin as u16),
            style.line as u16,
        );
        ui::fill(canvas, line, Color::gray(190));
        let width = f32::from(self.status.width) - 2.0 * style.margin as f32;
        let text = ui::fit(&font, style.small, env.i18n.tr("profiles.hint"), width);
        ui::draw_label(
            canvas,
            &font,
            style.small,
            style.margin as f32,
            self.status,
            &text,
            style.faint(60),
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            self.move_focus(-1, cx);
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            self.move_focus(1, cx);
        } else if *a == actions::NEXT_PAGE || *a == actions::PREV_PAGE {
            let rows = self.list.rows() as isize;
            self.move_focus(
                if *a == actions::NEXT_PAGE {
                    rows
                } else {
                    -rows
                },
                cx,
            );
        } else if *a == actions::CONFIRM {
            self.activate(self.list.focus, cx);
        } else if *a == actions::MENU {
            self.offer(self.list.focus, cx);
        } else if *a == actions::BACK {
            cx.command(Command::Back);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.rows().len();
        let paging = self.list.paging(len);
        if gesture.kind == GestureKind::Tap
            && let Some(action) = self
                .header
                .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, len) else {
            return Handled::No;
        };
        self.list.focus = index;
        match gesture.kind {
            GestureKind::Tap => self.activate(index, cx),
            GestureKind::LongPress => self.offer(index, cx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let index = params
            .get(params::PATH)
            .and_then(|p| p.parse::<usize>().ok())
            .filter(|&i| i < self.profiles.len());
        if *event == NAMED {
            let Some(name) = clean(params.get(params::TEXT).unwrap_or_default()) else {
                return;
            };
            self.profiles.retain(|p| p.name != name);
            self.profiles.push(Profile {
                values: profile::capture(&env),
                name: name.clone(),
            });
            profile::set_last(env.storage, &name);
            self.save(&env);
            self.list.focus = self.profiles.len() - 1;
            cx.command(Command::Notify(
                env.i18n
                    .format("profiles.saved", &[("name", Arg::Str(&name))]),
            ));
            self.redraw(cx);
        } else if *event == RENAMED {
            if let (Some(i), Some(name)) =
                (index, clean(params.get(params::TEXT).unwrap_or_default()))
            {
                self.profiles[i].name = name;
                self.save(&env);
                self.redraw(cx);
            }
        } else if *event == CHOSEN
            && let Some(i) = index
        {
            match params.get(choice::OPTION).unwrap_or_default() {
                "update" => {
                    self.profiles[i].values = profile::capture(&env);
                    self.save(&env);
                    let name = self.profiles[i].name.clone();
                    cx.command(Command::Notify(
                        env.i18n
                            .format("profiles.saved", &[("name", Arg::Str(&name))]),
                    ));
                    self.redraw(cx);
                }
                "rename" => {
                    let name = self.profiles[i].name.clone();
                    self.ask_name(cx, RENAMED, &name, &i.to_string());
                }
                "remove" => {
                    self.profiles.remove(i);
                    self.save(&env);
                    self.list.focus = self.list.focus.min(self.rows().len() - 1);
                    self.redraw(cx);
                }
                _ => {}
            }
        } else if *event == quireink_sdk::builtin::events::SETTING_CHANGED {
            // "In use" may have changed.
            self.redraw(cx);
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
