//! The list of catalogs: *Sync catalogs* (while some are marked for it),
//! the catalogs, and *Add catalog*. The line at the bottom tells how a
//! sync goes, else how to reach a catalog's options.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::choice;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::web::host;
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect,
    RefreshMode, Screen,
};

use crate::catalogs::{self, Catalog};
use crate::sync::{self, Report, Sync};
use crate::{BROWSE_SCREEN, CHOICE, EDIT_SCREEN, ID, OPTION_LABELS};

/// Replies: the address of a new catalog, a choice made.
const ADDRESS: Id = Id::new_static("opds.address");
const CHOSEN: Id = Id::new_static("opds.chosen");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Sync,
    Catalog(usize),
    Add,
}

pub(crate) struct ListScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// The line at the bottom.
    status: Rect,
    catalogs: Vec<Catalog>,
    sync: Rc<RefCell<Sync>>,
    message: Option<String>,
}

impl ListScreen {
    pub(crate) fn new(env: &Env<'_>, sync: Rc<RefCell<Sync>>) -> Self {
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
            catalogs: catalogs::load(env.storage),
            sync,
            message: None,
            style,
            header,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.catalogs.iter().any(|c| c.sync) || self.sync.borrow().running() {
            rows.push(Row::Sync);
        }
        rows.extend((0..self.catalogs.len()).map(Row::Catalog));
        rows.push(Row::Add);
        rows
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    fn save(&self, env: &Env<'_>) {
        catalogs::save(env.storage, &self.catalogs);
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match self.rows().get(index).copied() {
            Some(Row::Sync) => {
                let mut sync = self.sync.borrow_mut();
                let mut out = Vec::new();
                if sync.running() {
                    sync.stop(&mut out);
                } else if !env.network {
                    drop(sync);
                    self.message = Some(env.i18n.tr("opds.no-network").to_string());
                } else {
                    sync.start(&env, crate::marked(&env), &mut out);
                }
                for command in out {
                    cx.command(command);
                }
                self.redraw(cx);
            }
            Some(Row::Catalog(i)) => cx.command(Command::OpenScreen(
                BROWSE_SCREEN,
                Params::new().with(ID, i.to_string()),
            )),
            Some(Row::Add) => cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                Params::new()
                    .with(params::TITLE, env.i18n.tr("opds.address").to_string())
                    .with(params::TEXT, "https://")
                    .with(params::REPLY, ADDRESS.as_str()),
            )),
            None => {}
        }
    }

    /// What can be done with row `index`.
    fn offer(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some(Row::Catalog(i)) = self.rows().get(index).copied() else {
            return self.activate(index, cx);
        };
        let title = self.catalogs[i].name.clone();
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &title,
            &i.to_string(),
            OPTION_LABELS,
            &["open", "sync", "edit", "remove"],
        ));
    }

    fn chosen(&mut self, context: &str, option: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(i) = context
            .parse::<usize>()
            .ok()
            .filter(|&i| i < self.catalogs.len())
        else {
            return;
        };
        match option {
            "open" => cx.command(Command::OpenScreen(
                BROWSE_SCREEN,
                Params::new().with(ID, i.to_string()),
            )),
            "sync" => {
                if !env.network {
                    self.message = Some(env.i18n.tr("opds.no-network").to_string());
                    return self.redraw(cx);
                }
                let mut out = Vec::new();
                let catalog = self.catalogs[i].clone();
                self.sync
                    .borrow_mut()
                    .start(&env, Vec::from([catalog]), &mut out);
                for command in out {
                    cx.command(command);
                }
                self.redraw(cx);
            }
            "edit" => cx.command(Command::OpenScreen(
                EDIT_SCREEN,
                Params::new().with(ID, i.to_string()),
            )),
            "remove" => {
                self.catalogs.remove(i);
                self.save(&env);
                let len = self.rows().len();
                self.list.focus = self.list.focus.min(len - 1);
                self.redraw(cx);
            }
            _ => {}
        }
    }

    /// The line at the bottom: a message, how a sync goes, or the hint.
    fn status_text(&self, env: &Env<'_>) -> String {
        if let Some(message) = &self.message {
            return message.clone();
        }
        match &self.sync.borrow().report {
            Some(report) => report_text(env, report),
            None => env.i18n.tr("opds.hint").to_string(),
        }
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        let len = self.rows().len();
        match self.list.move_focus(delta, len) {
            ListChange::None => {}
            ListChange::Page => self.redraw(cx),
            ListChange::Focus => {
                cx.request_redraw_area(self.list.area);
                cx.request_refresh(RefreshMode::Fast);
            }
        }
    }
}

/// How a sync goes or went.
pub(crate) fn report_text(env: &Env<'_>, report: &Report) -> String {
    let catalog = Arg::Str(&report.catalog);
    let mut text = if report.running {
        env.i18n.plural(
            "opds.syncing",
            report.loaded as i64,
            &[("catalog", catalog)],
        )
    } else {
        env.i18n.plural("opds.synced", report.loaded as i64, &[])
    };
    if report.failed > 0 {
        text.push(' ');
        text.push_str(&env.i18n.plural("opds.failed", report.failed as i64, &[]));
    }
    if report.only_lists > 0 {
        text.push(' ');
        text.push_str(
            &env.i18n
                .plural("opds.only-lists", report.only_lists as i64, &[]),
        );
    }
    if report.unreachable > 0 {
        text.push(' ');
        text.push_str(
            &env.i18n
                .plural("opds.unreachable", report.unreachable as i64, &[]),
        );
    }
    text
}

impl Screen for ListScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("opds.title"),
            None,
            self.list.paging(rows.len()),
        );
        let catalogs = &self.catalogs;
        let running = self.sync.borrow().running();
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| {
                let (title, value, note) = match rows[index] {
                    Row::Sync if running => (
                        env.i18n.tr("opds.stop").to_string(),
                        String::new(),
                        env.i18n.tr("opds.stop.help").to_string(),
                    ),
                    Row::Sync => (
                        env.i18n.tr("opds.sync-all").to_string(),
                        String::new(),
                        env.i18n.tr("opds.sync-all.help").to_string(),
                    ),
                    Row::Catalog(i) => {
                        let catalog = &catalogs[i];
                        let mut note = host(&catalog.url).to_string();
                        if catalog.sync {
                            let folder = catalog.sync_folder(env);
                            note = env.i18n.format(
                                "opds.synced-to",
                                &[("host", Arg::Str(&note)), ("folder", Arg::Str(&folder))],
                            );
                        }
                        (catalog.name.clone(), String::from("›"), note)
                    }
                    Row::Add => (
                        env.i18n.tr("opds.add").to_string(),
                        String::from("+"),
                        String::new(),
                    ),
                };
                ui::draw_noted_row(canvas, env, &style, rect, focused, &title, &value, &note);
            },
        );
        ui::fill(canvas, self.status, Color::WHITE);
        if let Some(font) = style.font(env) {
            let line = Rect::new(
                self.status.x + style.margin as u16,
                self.status.y,
                self.status.width.saturating_sub(2 * style.margin as u16),
                style.line as u16,
            );
            ui::fill(canvas, line, Color::gray(190));
            let text = self.status_text(env);
            let width = f32::from(self.status.width) - 2.0 * style.margin as f32;
            let text = ui::fit(&font, style.small, &text, width);
            ui::draw_label(
                canvas,
                &font,
                style.small,
                style.margin as f32,
                self.status,
                &text,
                Color::gray(60),
            );
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        // Back from editing a catalog.
        self.catalogs = catalogs::load(cx.env().storage);
        let len = self.rows().len();
        self.list.focus = self.list.focus.min(len - 1);
        self.redraw(cx);
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
        self.message = None;
        match gesture.kind {
            GestureKind::Tap => self.activate(index, cx),
            GestureKind::LongPress => self.offer(index, cx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if *event == ADDRESS {
            let text = params.get(params::TEXT).unwrap_or_default().trim();
            if text.is_empty() || text == "https://" {
                return;
            }
            let url = if text.contains("://") {
                text.to_string()
            } else {
                format!("https://{text}")
            };
            self.catalogs.push(Catalog::new(&url));
            self.save(&env);
            let i = self.catalogs.len() - 1;
            cx.command(Command::OpenScreen(
                EDIT_SCREEN,
                Params::new().with(ID, i.to_string()),
            ));
        } else if *event == CHOSEN {
            let context = params.get(params::PATH).unwrap_or_default().to_string();
            let option = params.get(choice::OPTION).unwrap_or_default().to_string();
            self.chosen(&context, &option, cx);
        } else if *event == sync::CHANGED {
            self.message = None;
            self.redraw(cx);
        }
    }
}
