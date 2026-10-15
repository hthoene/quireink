//! Typing on a phone (slot `core.remote-keyboard`): the device serves a
//! small page with a text field to the local network and shows its
//! address as a QR code; what is sent from there comes back to the
//! keyboard. Handy for passwords and keys, which are tedious to type on
//! an e-reader.
//!
//! The address carries a key made anew each time; the page is served
//! only while the screen is open, and the screen closes once the text
//! has come.

use alloc::format;
use alloc::string::{String, ToString};

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::qr::QrCode;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, ServerResponse, server_params as p,
};

use crate::{PORT, key, web};

const SERVER: Id = Id::new_static("transfer.phone-server");
const SERVER_EVENT: Id = Id::new_static("transfer.phone-server-event");
/// Longest text taken, in bytes.
const MAX_TEXT: usize = 4096;

/// The web page; its texts come from the translations.
const PAGE: &str = include_str!("phone.html");

#[derive(Debug)]
enum State {
    Starting,
    Serving { url: String, qr: Option<QrCode> },
    Failed(String),
}

pub(crate) struct PhoneScreen {
    style: UiStyle,
    header: ui::Header,
    /// The keyboard's parameters, sent back with the text.
    params: Params,
    state: State,
    key: String,
    page: String,
    started: bool,
}

impl PhoneScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        Self {
            header: ui::Header::new(env, &style, true),
            style,
            params: params.clone(),
            state: State::Starting,
            key: key(env),
            page: page(env, params),
            started: false,
        }
    }

    fn content(&self, env: &Env<'_>) -> Rect {
        let display = env.device.display;
        let top = self.header.area.height + self.style.gap as u16;
        Rect::new(0, top, display.width, display.height.saturating_sub(top))
    }

    fn on_server_event(&mut self, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match params.get(p::KIND).unwrap_or_default() {
            p::STARTED => {
                let address = params.get(p::ADDRESS).unwrap_or_default();
                let url = format!("http://{address}/?k={}", self.key);
                let qr = QrCode::new(url.as_bytes());
                self.state = State::Serving { url, qr };
            }
            p::FAILED => {
                let reason = params.get(p::REASON).unwrap_or_default();
                self.state = State::Failed(if reason == "no network" {
                    env.i18n.tr("transfer.no-network").to_string()
                } else {
                    env.i18n
                        .format("transfer.failed", &[("reason", Arg::Str(reason))])
                });
            }
            p::REQUEST => {
                let number = params
                    .get(p::REQUEST)
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let request = Request {
                    method: params.get(p::METHOD).unwrap_or_default(),
                    target: params.get(p::TARGET).unwrap_or_default(),
                    body: params
                        .get(p::BODY_FILE)
                        .and_then(|file| quireink_sdk::read_file(env.storage, file, MAX_TEXT).ok()),
                };
                let (response, text) = answer(number, &self.key, &self.page, &request);
                cx.command(Command::Respond(response));
                // The text to the keyboard, and done.
                if let Some(text) = text
                    && let Some(reply) = self
                        .params
                        .get(params::REPLY)
                        .and_then(|r| Id::parse(r).ok())
                {
                    cx.command(Command::Back);
                    cx.command(Command::Emit(
                        reply,
                        self.params.clone().with(params::TEXT, text),
                    ));
                }
                return;
            }
            _ => return,
        }
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }
}

/// A request to the page.
pub(crate) struct Request<'a> {
    pub method: &'a str,
    pub target: &'a str,
    pub body: Option<alloc::vec::Vec<u8>>,
}

/// The answer to `request`, and the text sent, if that is what it did.
pub(crate) fn answer(
    number: u32,
    key: &str,
    page: &str,
    request: &Request<'_>,
) -> (ServerResponse, Option<String>) {
    let (path, query) = request
        .target
        .split_once('?')
        .unwrap_or((request.target, ""));
    let response =
        |status, kind: &str, body: &[u8]| ServerResponse::new(number, status, kind, body.to_vec());
    if web::param(query, "k").as_deref() != Some(key) {
        let text = b"The key is missing or wrong: scan the code shown on the device.";
        return (response(403, "text/plain; charset=utf-8", text), None);
    }
    if request.method == "POST" && path == "/send" {
        let text = request
            .body
            .as_deref()
            .map(|data| String::from_utf8_lossy(data).into_owned())
            .unwrap_or_default();
        return (
            response(200, "application/json", b"{\"ok\":true}"),
            Some(text),
        );
    }
    if path == "/" {
        (
            response(200, "text/html; charset=utf-8", page.as_bytes()),
            None,
        )
    } else {
        (
            response(404, "text/plain; charset=utf-8", b"not found"),
            None,
        )
    }
}

/// The page with its texts and what the keyboard asked for.
fn page(env: &Env<'_>, params: &Params) -> String {
    let texts = [
        ("send", "transfer.phone.send"),
        ("sent", "transfer.phone.sent"),
        ("failed", "transfer.web.failed"),
        ("show", "transfer.phone.show"),
    ];
    let mut json = String::from("{");
    for (name, key) in texts {
        json.push_str(&format!("{name}:{},", web::json_string(env.i18n.tr(key))));
    }
    let title = params.get(params::TITLE).unwrap_or_default();
    let title = if title.is_empty() {
        env.i18n.tr("transfer.phone.title")
    } else {
        title
    };
    let password = params.get(params::MODE) == Some("password");
    json.push_str(&format!(
        "title:{},text:{},password:{password}}}",
        web::json_string(title),
        web::json_string(if password {
            ""
        } else {
            params.get(params::TEXT).unwrap_or_default()
        }),
    ));
    PAGE.replace("/*STRINGS*/{}", &json)
}

impl Screen for PhoneScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let title = env.i18n.tr("transfer.phone.title");
        self.header.draw(canvas, env, &style, title, None, None);
        let area = self.content(env);
        match &self.state {
            State::Starting => {
                ui::draw_message(canvas, env, &style, area, env.i18n.tr("transfer.starting"))
            }
            State::Failed(message) => ui::draw_message(canvas, env, &style, area, message),
            State::Serving { url, qr } => {
                let hint = env.i18n.tr("transfer.phone.hint");
                crate::draw_address(canvas, env, &style, area, url, qr.as_ref(), hint, None);
            }
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if !self.started {
            self.started = true;
            cx.command(Command::Serve {
                server: SERVER,
                port: PORT,
                events: SERVER_EVENT,
            });
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if *action == actions::BACK {
            cx.command(Command::Back);
            Handled::Yes
        } else {
            Handled::No
        }
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind == GestureKind::Tap
            && let Some(action) = self.header.tap(cx.env(), &self.style, gesture.start, None)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == SERVER_EVENT && params.get(params::ID) == Some(SERVER.as_str()) {
            self.on_server_event(params, cx);
        }
    }
}
