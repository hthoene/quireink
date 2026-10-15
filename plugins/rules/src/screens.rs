//! The list of rules, and the list to pick an action or a switch from.
//!
//! Adding a rule asks, one after the other: when (a choice; for a time of
//! day, the time on the keyboard), what (a choice: run an action, turn a
//! switch on or off), then which action or switch ([`PickScreen`]).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::settings::SCOPE_ONLY;
use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::choice;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, SettingKind,
};

use crate::rule::{self, Effect, Rule, Trigger};
use crate::{CHOICE, PICK_SCREEN, effect_text, trigger_text};

/// Replies: a trigger, a time, an effect, an action or switch picked, a
/// rule's option.
const TRIGGER_CHOSEN: Id = Id::new_static("rules.trigger-chosen");
const TIME_ENTERED: Id = Id::new_static("rules.time-entered");
const EFFECT_CHOSEN: Id = Id::new_static("rules.effect-chosen");
const PICKED: Id = Id::new_static("rules.picked");
const OPTION_CHOSEN: Id = Id::new_static("rules.option-chosen");
/// Picking an action, or a switch.
const ACTION: &str = "action";
const SWITCH: &str = "switch";

/// A rule being added: when, then what kind of effect.
#[derive(Debug, Clone, Copy, Default)]
struct Draft {
    trigger: Option<Trigger>,
    /// `action`, `switch-on` or `switch-off`.
    effect: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Rule(usize),
    Add,
}

pub(crate) struct RulesScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// The line at the bottom.
    status: Rect,
    rules: Vec<Rule>,
    draft: Draft,
    message: Option<String>,
}

impl RulesScreen {
    pub(crate) fn new(env: &Env<'_>) -> Self {
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
            rules: rule::load(env.storage),
            draft: Draft::default(),
            message: None,
            style,
            header,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = (0..self.rules.len()).map(Row::Rule).collect();
        rows.push(Row::Add);
        rows
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    /// Keeps the rules and sets the timer again.
    fn save(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        rule::save(env.storage, &self.rules);
        let mut out = Vec::new();
        crate::schedule(&env, &mut out);
        for command in out {
            cx.command(command);
        }
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match self.rows().get(index).copied() {
            Some(Row::Rule(i)) => {
                self.rules[i].on = !self.rules[i].on;
                self.save(cx);
                self.redraw(cx);
            }
            Some(Row::Add) => {
                self.draft = Draft::default();
                cx.command(choice::open(
                    CHOICE,
                    TRIGGER_CHOSEN,
                    env.i18n.tr("rules.when"),
                    "",
                    "rules.trigger.",
                    &["start", "time", "book", "resume"],
                ));
            }
            None => {}
        }
    }

    fn offer(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(Row::Rule(i)) = self.rows().get(index).copied() else {
            return self.activate(index, cx);
        };
        let title = trigger_text(&env, self.rules[i].trigger);
        cx.command(choice::open(
            CHOICE,
            OPTION_CHOSEN,
            &title,
            &i.to_string(),
            "rules.option.",
            &["remove"],
        ));
    }

    /// Asks what the new rule does.
    fn ask_effect(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        cx.command(choice::open(
            CHOICE,
            EFFECT_CHOSEN,
            env.i18n.tr("rules.what"),
            "",
            "rules.effect.",
            &["action", "switch-on", "switch-off"],
        ));
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        let len = self.rows().len();
        if self.list.move_focus(delta, len) != ListChange::None {
            self.redraw(cx);
        }
    }
}

impl Screen for RulesScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let rows = self.rows();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("rules.title"),
            None,
            self.list.paging(rows.len()),
        );
        let rules = &self.rules;
        self.list.draw(
            canvas,
            &style,
            rows.len(),
            |canvas, index, rect, focused| match rows[index] {
                Row::Rule(i) => {
                    let rule = &rules[i];
                    let title = trigger_text(env, rule.trigger);
                    let note = effect_text(env, &rule.effect);
                    ui::draw_switch_row(canvas, env, &style, rect, focused, &title, &note, rule.on);
                }
                Row::Add => {
                    let title = env.i18n.tr("rules.add");
                    ui::draw_noted_row(canvas, env, &style, rect, focused, title, "+", "");
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
        let text = self
            .message
            .clone()
            .unwrap_or_else(|| env.i18n.tr("rules.hint").to_string());
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

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        self.rules = rule::load(cx.env().storage);
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
        let option = params.get(choice::OPTION).unwrap_or_default();
        if *event == TRIGGER_CHOSEN {
            self.draft.trigger = match option {
                "start" => Some(Trigger::Start),
                "book" => Some(Trigger::BookOpened),
                "resume" => Some(Trigger::Resume),
                "time" => {
                    cx.command(Command::OpenSlot(
                        slots::KEYBOARD,
                        Params::new()
                            .with(params::TITLE, env.i18n.tr("rules.time-title").to_string())
                            .with(params::TEXT, "22:00")
                            .with(params::REPLY, TIME_ENTERED.as_str()),
                    ));
                    return;
                }
                _ => return,
            };
            self.ask_effect(cx);
        } else if *event == TIME_ENTERED {
            match rule::parse_time(params.get(params::TEXT).unwrap_or_default()) {
                Some(minute) => {
                    self.draft.trigger = Some(Trigger::Time(minute));
                    self.ask_effect(cx);
                }
                None => cx.command(Command::Notify(env.i18n.tr("rules.bad-time").to_string())),
            }
        } else if *event == EFFECT_CHOSEN {
            let (effect, kind) = match option {
                "action" => ("action", ACTION),
                "switch-on" => ("switch-on", SWITCH),
                "switch-off" => ("switch-off", SWITCH),
                _ => return,
            };
            self.draft.effect = Some(effect);
            cx.command(Command::OpenScreen(
                PICK_SCREEN,
                Params::new()
                    .with(params::MODE, kind)
                    .with(params::REPLY, PICKED.as_str()),
            ));
        } else if *event == PICKED {
            let (Some(trigger), Some(kind), Some(id)) = (
                self.draft.trigger,
                self.draft.effect,
                params.get(params::ID).and_then(|id| Id::parse(id).ok()),
            ) else {
                return;
            };
            let effect = match kind {
                "action" => Effect::Action(id),
                "switch-on" => Effect::Switch(id, true),
                _ => Effect::Switch(id, false),
            };
            self.draft = Draft::default();
            let mut out = Vec::new();
            crate::add(
                &env,
                Rule {
                    trigger,
                    effect,
                    on: true,
                },
                &mut out,
            );
            for command in out {
                cx.command(command);
            }
            self.rules = rule::load(env.storage);
            self.list.focus = self.rules.len().saturating_sub(1);
            self.redraw(cx);
        } else if *event == OPTION_CHOSEN && option == "remove" {
            let at = params
                .get(params::PATH)
                .and_then(|p| p.parse::<usize>().ok());
            if let Some(i) = at.filter(|&i| i < self.rules.len()) {
                self.rules.remove(i);
                self.save(cx);
                self.list.focus = self.list.focus.min(self.rows().len() - 1);
                self.redraw(cx);
            }
        }
    }
}

/// Picks an action (parameter `mode` = `action`) or a switch (`switch`):
/// every one by its name, in alphabetical order. The choice comes back
/// as the event named by `reply`, with `id`.
pub(crate) struct PickScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    title: String,
    /// Ids and names.
    entries: Vec<(Id, String)>,
    reply: Option<Id>,
}

impl PickScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let switches = params.get(params::MODE) == Some(SWITCH);
        let mut entries: Vec<(Id, String)> = if switches {
            env.settings
                .specs()
                // "Only this book" means nothing outside a book.
                .filter(|s| matches!(s.kind, SettingKind::Bool) && s.id != SCOPE_ONLY)
                .map(|s| (s.id.clone(), env.i18n.tr(&s.title).to_string()))
                .collect()
        } else {
            env.actions
                .iter()
                .map(|a| (a.id.clone(), env.i18n.tr(&a.title).to_string()))
                .collect()
        };
        entries.sort_by_key(|e| e.1.to_lowercase());
        entries.dedup_by(|a, b| a.0 == b.0);
        let title = env
            .i18n
            .tr(if switches {
                "rules.pick.switch"
            } else {
                "rules.pick.action"
            })
            .to_string();
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            title,
            entries,
            reply: params.get(params::REPLY).and_then(|r| Id::parse(r).ok()),
            style,
            header,
        }
    }

    fn pick(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let (Some((id, _)), Some(reply)) = (self.entries.get(index), &self.reply) else {
            return;
        };
        cx.command(Command::Back);
        cx.command(Command::Emit(
            reply.clone(),
            Params::new().with(params::ID, id.as_str().to_string()),
        ));
    }
}

impl Screen for PickScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.entries.len();
        self.header.draw(
            canvas,
            env,
            &style,
            &self.title,
            None,
            self.list.paging(len),
        );
        let entries = &self.entries;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                ui::draw_value_row(canvas, env, &style, rect, focused, &entries[index].1, "");
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let len = self.entries.len();
        let rows = self.list.rows() as isize;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.pick(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.list.move_focus(delta, len) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let len = self.entries.len();
        let paging = self.list.paging(len);
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, len) {
            Some(index) => {
                self.list.focus = index;
                self.pick(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}
