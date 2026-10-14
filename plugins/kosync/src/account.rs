//! The account screen: user name and password, then *Sign in* or *Create
//! account*; *Sign out* once signed in. A line below tells what happened.
//!
//! The password never leaves this screen: only its MD5 is sent and kept,
//! as KOReader does.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, SettingValue, http_params,
};

use crate::{ACCOUNT_CHANGED, USER, json, md5, save_key, server_request};

/// Replies of the keyboard and the server.
const USER_ENTERED: Id = Id::new_static("kosync.user-entered");
const PASSWORD_ENTERED: Id = Id::new_static("kosync.password-entered");
const AUTHORIZED: Id = Id::new_static("kosync.authorized");
const CREATED: Id = Id::new_static("kosync.created");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    User,
    Password,
    SignIn,
    Create,
    SignOut,
}

#[derive(Debug)]
pub(crate) struct AccountScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// Where the message goes.
    note: Rect,
    user: String,
    /// The MD5 of the password entered.
    key: Option<String>,
    /// The account signed in with (the setting).
    signed_in: String,
    /// Waiting for the server.
    busy: bool,
    message: String,
}

impl AccountScreen {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let rows = (style.row * 5) as u16;
        let list = ListView::new(Rect::new(0, top, display.width, rows), style.row);
        let note_top = top + rows + style.gap as u16;
        let note = Rect::new(
            0,
            note_top,
            display.width,
            // Right below the rows, a few lines high.
            ((style.row * 3) as u16).min(display.height.saturating_sub(note_top)),
        );
        let signed_in = env.settings.text(USER.as_str()).to_string();
        let mut screen = Self {
            style,
            header,
            list,
            note,
            user: signed_in.clone(),
            key: None,
            signed_in,
            busy: false,
            message: String::new(),
        };
        screen.message = screen.state_message(env);
        screen
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::from([Row::User, Row::Password, Row::SignIn, Row::Create]);
        if !self.signed_in.is_empty() {
            rows.push(Row::SignOut);
        }
        rows
    }

    /// What the line below says when nothing happened yet.
    fn state_message(&self, env: &Env<'_>) -> String {
        if !env.network {
            env.i18n.tr("kosync.no-network").to_string()
        } else if self.signed_in.is_empty() {
            env.i18n.tr("kosync.signed-out").to_string()
        } else {
            let user = Arg::Str(&self.signed_in);
            env.i18n.format("kosync.signed-in", &[("user", user)])
        }
    }

    fn show(&mut self, cx: &mut Cx<'_, '_>, message: String) {
        self.message = message;
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn activate(&mut self, row: Row, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match row {
            Row::User | Row::Password => {
                let (title, reply) = if row == Row::User {
                    ("kosync.user", USER_ENTERED)
                } else {
                    ("kosync.password", PASSWORD_ENTERED)
                };
                let mut request = Params::new()
                    .with(params::TITLE, env.i18n.tr(title).to_string())
                    .with(params::REPLY, reply.as_str());
                if row == Row::User {
                    request = request.with(params::TEXT, self.user.clone());
                } else {
                    request = request.with(params::MODE, "password");
                }
                cx.command(Command::OpenSlot(slots::KEYBOARD, request));
            }
            Row::SignIn | Row::Create => {
                if !env.network {
                    return self.show(cx, env.i18n.tr("kosync.no-network").to_string());
                }
                let Some(key) = self.key.clone().filter(|_| !self.user.is_empty()) else {
                    return self.show(cx, env.i18n.tr("kosync.missing").to_string());
                };
                let request = if row == Row::SignIn {
                    let auth = Some((self.user.as_str(), key.as_str()));
                    server_request(
                        &env,
                        "GET",
                        "/users/auth",
                        None,
                        AUTHORIZED,
                        Params::new(),
                        auth,
                    )
                } else {
                    let body = format!(
                        "{{\"username\":{},\"password\":{}}}",
                        json::string(&self.user),
                        json::string(&key)
                    );
                    server_request(
                        &env,
                        "POST",
                        "/users/create",
                        Some(body),
                        CREATED,
                        Params::new(),
                        None,
                    )
                };
                cx.command(Command::Http(request));
                self.busy = true;
                self.show(cx, env.i18n.tr("kosync.connecting").to_string());
            }
            Row::SignOut => {
                save_key(&env, None);
                self.signed_in.clear();
                self.key = None;
                cx.command(Command::SetSetting(
                    USER,
                    SettingValue::Text(Cow::Borrowed("")),
                ));
                cx.command(Command::Emit(ACCOUNT_CHANGED, Params::new()));
                self.list.focus = 0;
                let message = self.state_message(&env);
                self.show(cx, message);
            }
        }
    }

    /// The server's answer to signing in or creating the account.
    fn answered(&mut self, created: bool, status: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        self.busy = false;
        let ok = if created { "201" } else { "200" };
        if status == ok {
            let key = self.key.clone().unwrap_or_default();
            save_key(&env, Some(&key));
            self.signed_in = self.user.clone();
            let user = SettingValue::Text(Cow::Owned(self.user.clone()));
            cx.command(Command::SetSetting(USER, user));
            cx.command(Command::Emit(ACCOUNT_CHANGED, Params::new()));
            let message = self.state_message(&env);
            return self.show(cx, message);
        }
        let key = match status {
            "402" if created => "kosync.taken",
            _ => crate::failure(status),
        };
        let message = env.i18n.format(key, &[("status", Arg::Str(status))]);
        self.show(cx, message);
    }
}

fn moved(change: ListChange, cx: &mut Cx<'_, '_>) {
    if change != ListChange::None {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

impl Screen for AccountScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows();
        self.list.focus = self.list.focus.min(rows.len() - 1);
        let title = env.i18n.tr("kosync.group");
        self.header.draw(canvas, env, &style, title, None, None);
        let i18n = env.i18n;
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| {
                let (title, value) = match rows[index] {
                    Row::User => (i18n.tr("kosync.user"), self.user.clone()),
                    Row::Password => {
                        let dots = if self.key.is_some() {
                            "••••••"
                        } else {
                            ""
                        };
                        (i18n.tr("kosync.password"), dots.to_string())
                    }
                    Row::SignIn => (i18n.tr("kosync.sign-in"), "›".to_string()),
                    Row::Create => (i18n.tr("kosync.create"), "›".to_string()),
                    Row::SignOut => (i18n.tr("kosync.sign-out"), "›".to_string()),
                };
                ui::draw_value_row(canvas, env, &style, rect, focused, title, &value);
            },
        );
        ui::draw_message(canvas, env, &style, self.note, &self.message);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let rows = self.rows();
        let a = action;
        let delta = if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            -1
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            1
        } else if *a == actions::CONFIRM {
            if !self.busy {
                self.activate(rows[self.list.focus.min(rows.len() - 1)], cx);
            }
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        moved(self.list.move_focus(delta, rows.len()), cx);
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(action) = self.header.tap(cx.env(), &self.style, gesture.start, None) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let rows = self.rows();
        let Some(index) = self.list.hit(gesture.start, rows.len()) else {
            return Handled::No;
        };
        if index != self.list.focus {
            self.list.focus = index;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        if !self.busy {
            self.activate(rows[index], cx);
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let text = params.get(params::TEXT).unwrap_or_default();
        let status = params.get(http_params::STATUS).unwrap_or("0");
        if *event == USER_ENTERED {
            self.user = text.trim().to_string();
            // Next: the password.
            self.list.focus = 1;
        } else if *event == PASSWORD_ENTERED {
            self.key = (!text.is_empty()).then(|| md5::of(text));
            self.list.focus = 2;
        } else if *event == AUTHORIZED {
            return self.answered(false, status, cx);
        } else if *event == CREATED {
            return self.answered(true, status, cx);
        } else {
            return;
        }
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}
