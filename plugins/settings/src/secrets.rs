//! *Passwords and keys*: every password and key plugins keep (see
//! [`quireink_sdk::secrets`]) at a glance, never shown; a tap offers to
//! type it anew (on the keyboard or, where offered, on a phone) or to
//! remove it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::secrets::SecretChange;
use quireink_sdk::ui::{self, ListView, UiStyle};
use quireink_sdk::{Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Screen};

use crate::{ID, moved, navigate, page};

/// The list.
pub(crate) const SCREEN: Id = Id::new_static("settings.secrets");
/// What to do with one; parameter `id`.
pub(crate) const SECRET_SCREEN: Id = Id::new_static("settings.secret");
/// Opens the list.
pub(crate) const OPEN: Id = Id::new_static("settings.secrets.open");
/// Emitted by the keyboard with a secret typed anew (parameters `id` and
/// `text`).
pub(crate) const ENTERED: Id = Id::new_static("settings.secret-entered");

/// How a kept secret shows.
const HIDDEN: &str = "••••••";

/// Keeps a secret typed anew.
pub(crate) fn entered(env: &Env<'_>, p: &Params, out: &mut Vec<Command>) {
    let (Some(id), Some(text)) = (p.get(ID), p.get(params::TEXT)) else {
        return;
    };
    let text = text.trim();
    if !text.is_empty() && env.secrets.get(id).is_some() {
        out.push(Command::SetSecret(SecretChange::keep(id, "", text)));
    }
}

/// The list of secrets.
pub(crate) struct SecretList {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
}

impl SecretList {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let (style, header, area) = page(env);
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
        }
    }

    fn open(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let id = cx.env().secrets.iter().nth(index).map(|s| s.id.clone());
        if let Some(id) = id {
            cx.command(Command::OpenScreen(
                SECRET_SCREEN,
                Params::new().with(ID, id),
            ));
        }
    }
}

impl Screen for SecretList {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = env.secrets.iter().count();
        let title = env.i18n.tr("settings.secrets");
        self.header
            .draw(canvas, env, &style, title, None, self.list.paging(len));
        if len == 0 {
            let area = self.list.area;
            ui::draw_message(
                canvas,
                env,
                &style,
                area,
                env.i18n.tr("settings.secrets.none"),
            );
            return;
        }
        let secrets: Vec<_> = env.secrets.iter().collect();
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let label = env.i18n.tr(&secrets[index].label);
                ui::draw_value_row(canvas, env, &style, rect, focused, label, HIDDEN);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = cx.env().secrets.iter().count();
        if let Some(change) = navigate(&mut self.list, len, action) {
            moved(change, cx);
            return Handled::Yes;
        }
        if *action == actions::CONFIRM {
            self.open(self.list.focus, cx);
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let len = cx.env().secrets.iter().count();
        let paging = self.list.paging(len);
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, len) {
            Some(index) => {
                self.list.focus = index;
                self.open(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}

/// What can be done with one secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Change,
    Remove,
}

/// Change or remove one secret.
pub(crate) struct SecretPage {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    id: String,
    title: String,
    choices: Vec<Choice>,
}

impl SecretPage {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let (style, header, area) = page(env);
        let id = params.get(ID).unwrap_or_default().to_string();
        let secret = env.secrets.iter().find(|s| s.id == id);
        let title = secret.map_or_else(String::new, |s| env.i18n.tr(&s.label).to_string());
        let mut choices = Vec::new();
        if secret.is_some_and(|s| s.changeable) {
            choices.push(Choice::Change);
        }
        choices.push(Choice::Remove);
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
            id,
            title,
            choices,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        match self.choices.get(index) {
            Some(Choice::Change) => cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                Params::new()
                    .with(params::TITLE, self.title.clone())
                    .with(params::MODE, "password")
                    .with(params::REPLY, ENTERED.as_str())
                    .with(ID, self.id.clone()),
            )),
            Some(Choice::Remove) => {
                cx.command(Command::SetSecret(SecretChange::remove(&self.id)));
                cx.command(Command::Back);
            }
            None => {}
        }
    }
}

impl Screen for SecretPage {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.choices.len();
        self.header.draw(
            canvas,
            env,
            &style,
            &self.title,
            None,
            self.list.paging(len),
        );
        let choices = &self.choices;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let key = match choices[index] {
                    Choice::Change => "settings.secret.change",
                    Choice::Remove => "settings.secret.remove",
                };
                ui::draw_value_row(canvas, env, &style, rect, focused, env.i18n.tr(key), "");
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if let Some(change) = navigate(&mut self.list, self.choices.len(), action) {
            moved(change, cx);
            return Handled::Yes;
        }
        if *action == actions::CONFIRM {
            self.choose(self.list.focus, cx);
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = self.list.paging(self.choices.len());
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.choices.len()) {
            Some(index) => {
                self.list.focus = index;
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        // Typed anew: back to the list.
        if *event == ENTERED {
            cx.command(Command::Back);
        }
    }
}
