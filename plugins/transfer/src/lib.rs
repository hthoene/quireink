//! Transfer by browser: while the screen is open, the device serves a
//! web page to the local network for putting books and other files on
//! it, fetching them and tidying up — from a phone or a computer, no
//! cable or app needed.
//!
//! The screen shows the page's address as text and as a QR code. The
//! address carries a key made anew each time, so others in the network
//! cannot get at the files. Closing the screen ends the transfer.

#![no_std]

extern crate alloc;

mod web;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus, params};
use quireink_sdk::qr::QrCode;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, Version, server_params as p,
};

pub const PLUGIN: Id = Id::new_static("transfer");
/// Opens the transfer screen.
pub const OPEN: Id = Id::new_static("transfer.open");
const SCREEN: Id = Id::new_static("transfer.screen");
const SERVER: Id = Id::new_static("transfer.server");
const SERVER_EVENT: Id = Id::new_static("transfer.server-event");

/// The port asked for; the next free one is taken if it is in use.
const PORT: u16 = 8080;

/// Lines of the hint below the code, at most.
const HINT_LINES: usize = 5;

/// The web page; its texts come from the translations.
const PAGE: &str = include_str!("page.html");

/// The transfer plugin.
#[derive(Debug, Default)]
pub struct Transfer;

impl Plugin for Transfer {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Transfer"),
            description: Cow::Borrowed(
                "Put books and other files on the device from a browser in the same network.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("transfer.title"),
            factory: Box::new(|env, _| Box::new(TransferScreen::new(env))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("transfer.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::MAIN,
            item: OPEN,
            order: 46,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("wifi"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[derive(Debug)]
enum State {
    Starting,
    /// Serving at `url`.
    Serving {
        url: String,
        qr: Option<QrCode>,
    },
    Failed(String),
}

struct TransferScreen {
    style: UiStyle,
    header: ui::Header,
    state: State,
    /// Part of the address, made anew for every transfer.
    key: String,
    /// The web page with its texts.
    page: String,
    /// What happened last.
    last: Option<String>,
    started: bool,
}

impl TransferScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        Self {
            header: ui::Header::new(env, &style, true),
            style,
            state: State::Starting,
            key: key(env),
            page: page(env),
            last: None,
            started: false,
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    /// The area below the header.
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
                self.redraw(cx);
            }
            p::FAILED => {
                let reason = params.get(p::REASON).unwrap_or_default();
                self.state = State::Failed(if reason == "no network" {
                    env.i18n.tr("transfer.no-network").to_string()
                } else {
                    env.i18n
                        .format("transfer.failed", &[("reason", Arg::Str(reason))])
                });
                self.redraw(cx);
            }
            p::REQUEST => {
                let request = web::Request {
                    number: params
                        .get(p::REQUEST)
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                    method: params.get(p::METHOD).unwrap_or_default(),
                    target: params.get(p::TARGET).unwrap_or_default(),
                    body_file: params.get(p::BODY_FILE),
                };
                let (response, done) = web::answer(env.storage, &self.key, &self.page, request);
                cx.command(Command::Respond(response));
                if let Some(done) = done {
                    let (key, name) = match &done {
                        web::Done::Received(name) => ("transfer.received", name),
                        web::Done::Sent(name) => ("transfer.sent", name),
                        web::Done::Folder(name) => ("transfer.folder", name),
                        web::Done::Deleted(name) => ("transfer.deleted", name),
                    };
                    self.last = Some(env.i18n.format(key, &[("name", Arg::Str(name))]));
                    let area = self.content(&env);
                    cx.request_redraw_area(area);
                    cx.request_refresh(RefreshMode::Fast);
                }
            }
            _ => {}
        }
    }
}

/// A key for this transfer: hard to guess for others in the network,
/// short enough to type.
fn key(env: &Env<'_>) -> String {
    let mut hash = quireink_sdk::md5::Md5::new();
    hash.update(env.device.id.as_bytes());
    hash.update(&env.now.0.to_le_bytes());
    if let Some(seconds) = quireink_sdk::time::unix_seconds(env) {
        hash.update(&seconds.to_le_bytes());
    }
    hash.update(&env.status.battery.map_or(0, |b| b.percent).to_le_bytes());
    // Where memory is differs from run to run on most systems.
    let probe = Box::new(0u8);
    hash.update(&(&raw const *probe as usize).to_le_bytes());
    let mut key = hash.hex();
    key.truncate(8);
    key
}

/// The web page, with its texts in the device's language.
fn page(env: &Env<'_>) -> String {
    let texts = [
        ("title", "transfer.web.title"),
        ("device", "transfer.web.device"),
        ("upload", "transfer.web.upload"),
        ("drop", "transfer.web.drop"),
        ("newFolder", "transfer.web.new-folder"),
        ("folderName", "transfer.web.folder-name"),
        ("delete", "transfer.web.delete"),
        ("deleteQuestion", "transfer.web.delete-question"),
        ("empty", "transfer.web.empty"),
        ("uploading", "transfer.web.uploading"),
        ("done", "transfer.web.done"),
        ("failed", "transfer.web.failed"),
    ];
    let mut json = String::from("{");
    for (i, (name, key)) in texts.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!("{name}:{}", web::json_string(env.i18n.tr(key))));
    }
    json.push('}');
    PAGE.replace("/*STRINGS*/{}", &json)
}

impl Screen for TransferScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let title = env.i18n.tr("transfer.title");
        self.header.draw(canvas, env, &style, title, None, None);
        let area = self.content(env);
        let (url, qr) = match &self.state {
            State::Starting => {
                return ui::draw_message(
                    canvas,
                    env,
                    &style,
                    area,
                    env.i18n.tr("transfer.starting"),
                );
            }
            State::Failed(message) => {
                return ui::draw_message(canvas, env, &style, area, message);
            }
            State::Serving { url, qr } => (url, qr),
        };
        let Some(font) = style.font(env) else {
            return;
        };
        let margin = style.margin as u16;
        let line = font.line_metrics(style.text).line_height as u16;
        let small = font.line_metrics(style.small).line_height as u16;
        // Below the code: the address, the hint (up to five lines) and
        // what happened last.
        let hint = env.i18n.tr("transfer.hint");
        let width = f32::from(area.width - 2 * margin);
        let hint_lines = font.wrap(style.small, hint, width);
        let text_height =
            line + small * (hint_lines.len().min(HINT_LINES) as u16 + 1) + style.gap as u16 * 3;
        let room = area.height.saturating_sub(text_height + style.gap as u16);
        let side = room.min(area.width - 2 * margin);
        let mut y = area.y;
        if let Some(qr) = qr {
            // Four modules of white around the code, as scanners expect.
            let modules = qr.size() as u16 + 8;
            let scale = (side / modules).max(1);
            let size = scale * modules;
            let x0 = (area.width - size) / 2 + 4 * scale;
            let y0 = area.y + (side - size) / 2 + 4 * scale;
            for my in 0..qr.size() {
                for mx in 0..qr.size() {
                    if qr.dark(mx, my) {
                        canvas.fill_rect(
                            i32::from(x0 + mx as u16 * scale),
                            i32::from(y0 + my as u16 * scale),
                            u32::from(scale),
                            u32::from(scale),
                            Color::BLACK,
                        );
                    }
                }
            }
            y += side;
        }
        y += style.gap as u16;
        let shown = url.strip_prefix("http://").unwrap_or(url);
        let row = |y: u16, height: u16| Rect::new(margin, y, area.width - 2 * margin, height);
        let centered = |text: &str, size: f32| {
            let text = ui::fit(&font, size, text, width);
            let x = f32::from(margin) + (width - font.measure(size, &text)) / 2.0;
            (text, x.max(f32::from(margin)))
        };
        let (text, x) = centered(shown, style.text);
        ui::draw_label(
            canvas,
            &font,
            style.text,
            x,
            row(y, line),
            &text,
            Color::BLACK,
        );
        y += line + style.gap as u16;
        for range in hint_lines.into_iter().take(HINT_LINES) {
            let (text, x) = centered(&hint[range], style.small);
            ui::draw_label(
                canvas,
                &font,
                style.small,
                x,
                row(y, small),
                &text,
                Color::gray(60),
            );
            y += small;
        }
        y += style.gap as u16;
        if let Some(last) = &self.last {
            let (text, x) = centered(last, style.small);
            ui::draw_label(
                canvas,
                &font,
                style.small,
                x,
                row(y, small),
                &text,
                Color::BLACK,
            );
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
            // Closing the screen also ends the transfer.
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

#[cfg(test)]
mod tests;
