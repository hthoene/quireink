//! The list of rules, and the list to pick an action or a switch from.
//!
//! Adding a rule asks, one after the other: when (a choice; for a time of
//! day the time on the keyboard and the days, for a repeat the minutes,
//! for the battery the percentage, for a switch which one), what (a
//! choice: run an action, turn a switch on or off, set a number, show a
//! message), then which action, switch or number ([`PickScreen`]) and
//! its value, or the message. A rule's options add a condition (only
//! if …): between two times, while reading or not, on battery or while
//! charging, while a switch is on or off.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::settings::SCOPE_ONLY;
use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::choice;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect,
    RefreshMode, Screen, SettingKind,
};

use crate::rule::{self, Condition, EVERY_DAY, Effect, Rule, Trigger, WEEKDAYS, WEEKENDS};
use crate::{CHOICE, PICK_SCREEN, effect_text, trigger_text, when_text};

/// Replies: a trigger, a time, an effect, an action or switch picked, a
/// rule's option.
const TRIGGER_CHOSEN: Id = Id::new_static("rules.trigger-chosen");
const TIME_ENTERED: Id = Id::new_static("rules.time-entered");
const DAYS_CHOSEN: Id = Id::new_static("rules.days-chosen");
const EVERY_ENTERED: Id = Id::new_static("rules.every-entered");
const BATTERY_ENTERED: Id = Id::new_static("rules.battery-entered");
/// The switch a trigger watches was picked.
const TRIGGER_PICKED: Id = Id::new_static("rules.trigger-picked");
/// The triggers offered, in this order.
const TRIGGERS: [&str; 13] = [
    "start",
    "time",
    "every",
    "book",
    "book-closed",
    "book-finished",
    "resume",
    "sleep",
    "battery",
    "charging",
    "unplugged",
    "switch-on",
    "switch-off",
];
/// The days a time of day may apply: every day, weekdays, weekends, or
/// one day (Monday first).
const DAYS: [&str; 10] = [
    "every", "weekdays", "weekends", "1", "2", "3", "4", "5", "6", "0",
];
const EFFECT_CHOSEN: Id = Id::new_static("rules.effect-chosen");
/// What a rule may do, in this order.
const EFFECTS: [&str; 5] = ["action", "switch-on", "switch-off", "number", "message"];
const PICKED: Id = Id::new_static("rules.picked");
const NUMBER_ENTERED: Id = Id::new_static("rules.number-entered");
const MESSAGE_ENTERED: Id = Id::new_static("rules.message-entered");
const OPTION_CHOSEN: Id = Id::new_static("rules.option-chosen");
/// A rule's condition was chosen; for times between, the first and the
/// second time entered; for a switch, which one.
const CONDITION_CHOSEN: Id = Id::new_static("rules.condition-chosen");
const FROM_ENTERED: Id = Id::new_static("rules.from-entered");
const TO_ENTERED: Id = Id::new_static("rules.to-entered");
const CONDITION_PICKED: Id = Id::new_static("rules.condition-picked");
/// The conditions offered, in this order.
const CONDITIONS: [&str; 8] = [
    "always",
    "between",
    "reading",
    "not-reading",
    "battery",
    "charging",
    "switch-on",
    "switch-off",
];
/// Picking an action, a switch or a number.
const ACTION: &str = "action";
const SWITCH: &str = "switch";
const NUMBER: &str = "number";

/// A rule being added: when, then what kind of effect.
#[derive(Debug, Clone, Default)]
struct Draft {
    trigger: Option<Trigger>,
    /// The time of day entered, until its days are chosen.
    minute: Option<u16>,
    /// The switch to watch is turned on (`true`) or off.
    watch_on: bool,
    /// `action`, `switch-on`, `switch-off` or `number`.
    effect: Option<&'static str>,
    /// The number picked, until its value is entered.
    number: Option<Id>,
    /// The rule whose condition is being chosen.
    editing: Option<usize>,
    /// The first time of a condition between two, until the second.
    from: Option<u16>,
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
                    &TRIGGERS,
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
        let title = trigger_text(&env, &self.rules[i].trigger);
        cx.command(choice::open(
            CHOICE,
            OPTION_CHOSEN,
            &title,
            &i.to_string(),
            "rules.option.",
            &["condition", "remove"],
        ));
    }

    /// Adds the rule drafted, doing `effect`.
    fn finish(&mut self, effect: Effect, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(trigger) = self.draft.trigger.clone() else {
            return;
        };
        self.draft = Draft::default();
        let mut out = Vec::new();
        crate::add(
            &env,
            Rule {
                trigger,
                effect,
                on: true,
                condition: Condition::Always,
            },
            &mut out,
        );
        for command in out {
            cx.command(command);
        }
        self.rules = rule::load(env.storage);
        self.list.focus = self.rules.len().saturating_sub(1);
        self.redraw(cx);
    }

    /// Gives the rule being edited `condition`.
    fn set_condition(&mut self, condition: Condition, cx: &mut Cx<'_, '_>) {
        let Some(i) = self.draft.editing.take().filter(|&i| i < self.rules.len()) else {
            return;
        };
        self.rules[i].condition = condition;
        self.save(cx);
        self.redraw(cx);
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
            &EFFECTS,
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
                    let title = when_text(env, rule);
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
            style.faint(60),
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
        let ask = |cx: &mut Cx<'_, '_>, title: &str, text: &str, reply: Id| {
            cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                Params::new()
                    .with(params::TITLE, env.i18n.tr(title).to_string())
                    .with(params::TEXT, text.to_string())
                    .with(params::REPLY, reply.as_str()),
            ));
        };
        if *event == TRIGGER_CHOSEN {
            self.draft.trigger = match option {
                "start" => Some(Trigger::Start),
                "book" => Some(Trigger::BookOpened),
                "book-closed" => Some(Trigger::BookClosed),
                "book-finished" => Some(Trigger::BookFinished),
                "resume" => Some(Trigger::Resume),
                "sleep" => Some(Trigger::Sleep),
                "charging" => Some(Trigger::Charging),
                "unplugged" => Some(Trigger::Unplugged),
                "time" => return ask(cx, "rules.time-title", "22:00", TIME_ENTERED),
                "every" => return ask(cx, "rules.every-title", "30", EVERY_ENTERED),
                "battery" => return ask(cx, "rules.battery-title", "20", BATTERY_ENTERED),
                "switch-on" | "switch-off" => {
                    self.draft.watch_on = option == "switch-on";
                    cx.command(Command::OpenScreen(
                        PICK_SCREEN,
                        Params::new()
                            .with(params::MODE, SWITCH)
                            .with(params::REPLY, TRIGGER_PICKED.as_str()),
                    ));
                    return;
                }
                _ => return,
            };
            self.ask_effect(cx);
        } else if *event == TIME_ENTERED {
            match rule::parse_time(params.get(params::TEXT).unwrap_or_default()) {
                Some(minute) => {
                    self.draft.minute = Some(minute);
                    cx.command(choice::open(
                        CHOICE,
                        DAYS_CHOSEN,
                        env.i18n.tr("rules.days-title"),
                        "",
                        "rules.days.",
                        &DAYS,
                    ));
                }
                None => cx.command(Command::Notify(env.i18n.tr("rules.bad-time").to_string())),
            }
        } else if *event == DAYS_CHOSEN {
            let days = match option {
                "every" => EVERY_DAY,
                "weekdays" => WEEKDAYS,
                "weekends" => WEEKENDS,
                day => match day.parse::<u8>() {
                    Ok(d) if d < 7 => 1 << d,
                    _ => return,
                },
            };
            if let Some(minute) = self.draft.minute.take() {
                self.draft.trigger = Some(Trigger::Time { minute, days });
                self.ask_effect(cx);
            }
        } else if *event == EVERY_ENTERED || *event == BATTERY_ENTERED {
            let number = params
                .get(params::TEXT)
                .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<u16>().ok());
            let trigger = match number {
                Some(m) if *event == EVERY_ENTERED && (1..=24 * 60).contains(&m) => {
                    Trigger::Every(m)
                }
                Some(p) if *event == BATTERY_ENTERED && (1..=100).contains(&p) => {
                    Trigger::BatteryBelow(p as u8)
                }
                _ => {
                    let text = env.i18n.tr("rules.bad-number").to_string();
                    cx.command(Command::Notify(text));
                    return;
                }
            };
            self.draft.trigger = Some(trigger);
            self.ask_effect(cx);
        } else if *event == TRIGGER_PICKED {
            let Some(id) = params.get(params::ID).and_then(|id| Id::parse(id).ok()) else {
                return;
            };
            self.draft.trigger = Some(if self.draft.watch_on {
                Trigger::SwitchOn(id)
            } else {
                Trigger::SwitchOff(id)
            });
            self.ask_effect(cx);
        } else if *event == EFFECT_CHOSEN {
            let (effect, kind) = match option {
                "action" => ("action", ACTION),
                "switch-on" => ("switch-on", SWITCH),
                "switch-off" => ("switch-off", SWITCH),
                "number" => ("number", NUMBER),
                "message" => return ask(cx, "rules.message-title", "", MESSAGE_ENTERED),
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
            let (Some(kind), Some(id)) = (
                self.draft.effect,
                params.get(params::ID).and_then(|id| Id::parse(id).ok()),
            ) else {
                return;
            };
            let effect = match kind {
                "action" => Effect::Action(id),
                "switch-on" => Effect::Switch(id, true),
                "switch-off" => Effect::Switch(id, false),
                _ => {
                    // A number: which value?
                    let Some((min, max)) = range(&env, &id) else {
                        return;
                    };
                    let title = env.i18n.format(
                        "rules.number-title",
                        &[("min", Arg::Int(min)), ("max", Arg::Int(max))],
                    );
                    let now = env.settings.int(id.as_str()).to_string();
                    self.draft.number = Some(id);
                    cx.command(Command::OpenSlot(
                        slots::KEYBOARD,
                        Params::new()
                            .with(params::TITLE, title)
                            .with(params::TEXT, now)
                            .with(params::REPLY, NUMBER_ENTERED.as_str()),
                    ));
                    return;
                }
            };
            self.finish(effect, cx);
        } else if *event == NUMBER_ENTERED {
            let Some(id) = self.draft.number.clone() else {
                return;
            };
            let value = params
                .get(params::TEXT)
                .and_then(|t| t.trim().trim_end_matches('%').trim().parse::<i64>().ok());
            match (value, range(&env, &id)) {
                (Some(v), Some((min, max))) if (min..=max).contains(&v) => {
                    self.finish(Effect::Number(id, v), cx);
                }
                _ => {
                    let text = env.i18n.tr("rules.bad-number").to_string();
                    cx.command(Command::Notify(text));
                }
            }
        } else if *event == MESSAGE_ENTERED {
            let text = params.get(params::TEXT).unwrap_or_default().trim();
            if !text.is_empty() {
                self.finish(Effect::Message(text.to_string()), cx);
            }
        } else if *event == OPTION_CHOSEN && option == "condition" {
            let at = params
                .get(params::PATH)
                .and_then(|p| p.parse::<usize>().ok());
            if let Some(i) = at.filter(|&i| i < self.rules.len()) {
                self.draft = Draft {
                    editing: Some(i),
                    ..Draft::default()
                };
                cx.command(choice::open(
                    CHOICE,
                    CONDITION_CHOSEN,
                    env.i18n.tr("rules.only-if"),
                    "",
                    "rules.condition.",
                    &CONDITIONS,
                ));
            }
        } else if *event == CONDITION_CHOSEN {
            let condition = match option {
                "always" => Condition::Always,
                "reading" => Condition::Reading,
                "not-reading" => Condition::NotReading,
                "battery" => Condition::OnBattery,
                "charging" => Condition::Charging,
                "between" => return ask(cx, "rules.from-title", "22:00", FROM_ENTERED),
                "switch-on" | "switch-off" => {
                    self.draft.watch_on = option == "switch-on";
                    cx.command(Command::OpenScreen(
                        PICK_SCREEN,
                        Params::new()
                            .with(params::MODE, SWITCH)
                            .with(params::REPLY, CONDITION_PICKED.as_str()),
                    ));
                    return;
                }
                _ => return,
            };
            self.set_condition(condition, cx);
        } else if *event == FROM_ENTERED || *event == TO_ENTERED {
            let Some(minute) = rule::parse_time(params.get(params::TEXT).unwrap_or_default())
            else {
                cx.command(Command::Notify(env.i18n.tr("rules.bad-time").to_string()));
                return;
            };
            if *event == FROM_ENTERED {
                self.draft.from = Some(minute);
                return ask(cx, "rules.to-title", "06:00", TO_ENTERED);
            }
            if let Some(from) = self.draft.from.take() {
                self.set_condition(Condition::Between { from, to: minute }, cx);
            }
        } else if *event == CONDITION_PICKED {
            let Some(id) = params.get(params::ID).and_then(|id| Id::parse(id).ok()) else {
                return;
            };
            let condition = if self.draft.watch_on {
                Condition::SwitchOn(id)
            } else {
                Condition::SwitchOff(id)
            };
            self.set_condition(condition, cx);
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

/// The range of number setting `id`.
fn range(env: &Env<'_>, id: &Id) -> Option<(i64, i64)> {
    env.settings.specs().find_map(|s| match s.kind {
        SettingKind::Int { min, max } if s.id == *id => Some((min, max)),
        _ => None,
    })
}

/// Picks an action (parameter `mode` = `action`), a switch (`switch`) or a
/// number (`number`): every one by its name, in alphabetical order. The choice comes back
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
        let mode = params.get(params::MODE).unwrap_or(ACTION);
        let mut entries: Vec<(Id, String)> = if mode == SWITCH || mode == NUMBER {
            env.settings
                .specs()
                // "Only this book" means nothing outside a book.
                .filter(|s| match s.kind {
                    SettingKind::Bool => mode == SWITCH && s.id != SCOPE_ONLY,
                    SettingKind::Int { .. } => mode == NUMBER,
                    _ => false,
                })
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
            .tr(match mode {
                SWITCH => "rules.pick.switch",
                NUMBER => "rules.pick.number",
                _ => "rules.pick.action",
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
