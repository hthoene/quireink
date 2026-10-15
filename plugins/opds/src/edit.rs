//! A catalog's details: its name, address, user name, password, the
//! folder its books are synced to, and whether *Sync catalogs* syncs it.
//! Every change is kept at once.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode, Screen,
};

use crate::ID;
use crate::catalogs::{self, Catalog};

/// The keyboard's reply with a field's new text.
const ENTERED: Id = Id::new_static("opds.entered");
/// Parameter of the reply: which field.
const FIELD: &str = "field";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Address,
    User,
    Password,
    Folder,
    Sync,
}

const FIELDS: [Field; 6] = [
    Field::Name,
    Field::Address,
    Field::User,
    Field::Password,
    Field::Folder,
    Field::Sync,
];

impl Field {
    fn key(self) -> &'static str {
        match self {
            Field::Name => "opds.field.name",
            Field::Address => "opds.field.address",
            Field::User => "opds.field.user",
            Field::Password => "opds.field.password",
            Field::Folder => "opds.field.folder",
            Field::Sync => "opds.field.sync",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Field::Name => "opds.field.name.help",
            Field::Address => "opds.field.address.help",
            Field::User => "opds.field.user.help",
            Field::Password => "opds.field.password.help",
            Field::Folder => "opds.field.folder.help",
            Field::Sync => "opds.field.sync.help",
        }
    }

    fn of(name: &str) -> Option<Field> {
        FIELDS.into_iter().find(|f| f.key() == name)
    }
}

pub(crate) struct EditScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    index: usize,
    catalog: Catalog,
}

impl EditScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let index = params
            .get(ID)
            .and_then(|i| i.parse().ok())
            .unwrap_or(usize::MAX);
        let catalog = catalogs::load(env.storage)
            .get(index)
            .cloned()
            .unwrap_or_default();
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            style,
            header,
            index,
            catalog,
        }
    }

    fn save(&self, env: &Env<'_>) {
        let mut all = catalogs::load(env.storage);
        if let Some(slot) = all.get_mut(self.index) {
            *slot = self.catalog.clone();
            catalogs::save(env.storage, &all);
        }
    }

    fn value(&self, env: &Env<'_>, field: Field) -> String {
        let c = &self.catalog;
        match field {
            Field::Name => c.name.clone(),
            Field::Address => c.url.clone(),
            Field::User => c.user.clone(),
            Field::Password if c.password.is_empty() => String::new(),
            Field::Password => String::from("••••"),
            Field::Folder => c.sync_folder(env),
            Field::Sync => String::new(),
        }
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(field) = FIELDS.get(index).copied() else {
            return;
        };
        if field == Field::Sync {
            self.catalog.sync = !self.catalog.sync;
            self.save(&env);
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return;
        }
        let mut request = Params::new()
            .with(params::TITLE, env.i18n.tr(field.key()).to_string())
            .with(params::REPLY, ENTERED.as_str())
            .with(FIELD, field.key());
        request = match field {
            Field::Password => request.with(params::MODE, "password"),
            Field::Folder => request.with(params::TEXT, self.catalog.sync_folder(&env)),
            _ => request.with(params::TEXT, self.value(&env, field)),
        };
        cx.command(Command::OpenSlot(slots::KEYBOARD, request));
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        match self.list.move_focus(delta, FIELDS.len()) {
            ListChange::None => {}
            _ => {
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
        }
    }
}

impl Screen for EditScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header.draw(
            canvas,
            env,
            &style,
            &self.catalog.name,
            None,
            self.list.paging(FIELDS.len()),
        );
        let values: Vec<String> = FIELDS.iter().map(|&f| self.value(env, f)).collect();
        let on = self.catalog.sync;
        self.list.draw(
            canvas,
            &style,
            FIELDS.len(),
            |canvas, index, rect, focused| {
                let field = FIELDS[index];
                let title = env.i18n.tr(field.key());
                let note = env.i18n.tr(field.help());
                if field == Field::Sync {
                    ui::draw_switch_row(canvas, env, &style, rect, focused, title, note, on);
                } else {
                    let value = &values[index];
                    ui::draw_noted_row(canvas, env, &style, rect, focused, title, value, note);
                }
            },
        );
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            self.move_focus(-1, cx);
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            self.move_focus(1, cx);
        } else if *a == actions::CONFIRM {
            self.activate(self.list.focus, cx);
        } else if *a == actions::BACK {
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
        let paging = self.list.paging(FIELDS.len());
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, FIELDS.len()) else {
            return Handled::No;
        };
        self.list.focus = index;
        self.activate(index, cx);
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event != ENTERED {
            return;
        }
        let env = *cx.env();
        let Some(field) = params.get(FIELD).and_then(Field::of) else {
            return;
        };
        let text = params.get(params::TEXT).unwrap_or_default();
        let trimmed = text.trim().to_string();
        let c = &mut self.catalog;
        match field {
            Field::Name if !trimmed.is_empty() => c.name = trimmed,
            Field::Address if !trimmed.is_empty() => c.url = trimmed,
            Field::User => c.user = trimmed,
            // Passwords may begin or end with spaces.
            Field::Password => c.password = text.to_string(),
            Field::Folder => c.folder = trimmed.trim_matches('/').to_string(),
            _ => return,
        }
        self.save(&env);
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}
