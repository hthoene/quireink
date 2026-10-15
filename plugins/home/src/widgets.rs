//! Widgets on the home screen: the ones chosen in the setting
//! [`WIDGETS`] (any plugin's, see `quireink_sdk::widgets`) below the
//! "continue reading" card, and the home screen's own clock with the date.
//! [`ChooseScreen`] chooses them; tapping one shows or hides it, shown
//! ones keep the order they were chosen in.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::status::item_ids;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::widgets::WidgetSpec;
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Rect, RefreshMode,
    RegisterError, Registrar, Screen, ScreenSpec, SettingKind, SettingSpec, SettingValue, time,
};

/// The widgets shown, ids separated by commas.
pub const WIDGETS: Id = Id::new_static("home.widgets");
/// Chooses them.
pub const CHOOSE_SCREEN: Id = Id::new_static("home.widgets-choose");
/// The clock with the date.
pub const CLOCK: Id = Id::new_static("home.clock");

/// The clock's digits are this much larger than titles.
const CLOCK_SCALE: f32 = 2.2;

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    r.add_setting(
        SettingSpec::new(
            WIDGETS,
            "home.setting.widgets",
            SettingKind::List,
            SettingValue::Text(Cow::Borrowed("")),
        )
        .group(quireink_sdk::builtin::groups::LOOK)
        .editor(CHOOSE_SCREEN)
        .description("home.setting.widgets.help"),
    )?;
    r.add_screen(ScreenSpec {
        id: CHOOSE_SCREEN,
        title: Cow::Borrowed("home.setting.widgets"),
        factory: Box::new(|env, _| Box::new(ChooseScreen::new(env))),
    })?;
    r.add_widget(WidgetSpec {
        id: CLOCK,
        title: Cow::Borrowed("home.widget.clock"),
        height: Box::new(|env, _| {
            let style = UiStyle::new(env);
            let Some(font) = style.font(env) else {
                return 0;
            };
            let big = font.line_metrics(style.title * CLOCK_SCALE).line_height;
            let small = font.line_metrics(style.small).line_height;
            (big + small) as u16 + style.gap as u16
        }),
        draw: Box::new(draw_clock),
        signature: Some(Box::new(|env| {
            format!(
                "{}|{}",
                time::clock_text(env).unwrap_or_default(),
                date(env)
            )
        })),
    })?;
    Ok(())
}

/// The date in words: "Wednesday, 15 October".
fn date(env: &Env<'_>) -> String {
    let Some(now) = time::local_time(env) else {
        return String::new();
    };
    env.i18n.format(
        "home.date",
        &[
            (
                "weekday",
                Arg::Str(env.i18n.tr(&format!("home.weekday.{}", now.weekday))),
            ),
            ("day", Arg::Int(i64::from(now.day))),
            (
                "month",
                Arg::Str(env.i18n.tr(&format!("home.month.{}", now.month))),
            ),
        ],
    )
}

fn draw_clock(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let size = style.title * CLOCK_SCALE;
    let big = font.line_metrics(size).line_height as u16;
    let clock = time::clock_text(env).unwrap_or_else(|| String::from("--:--"));
    let x = f32::from(area.x);
    ui::draw_label(
        canvas,
        &font,
        size,
        x,
        Rect::new(area.x, area.y, area.width, big),
        &clock,
        Color::BLACK,
    );
    let small = font.line_metrics(style.small).line_height as u16;
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x,
        Rect::new(area.x, area.y + big, area.width, small),
        &date(env),
        Color::gray(80),
    );
}

/// The ids of the widgets shown.
pub(crate) fn chosen<'e>(env: &Env<'e>) -> Vec<&'e str> {
    item_ids(env.settings.text(WIDGETS.as_str())).collect()
}

pub(crate) struct ChooseScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// Ids and the translation keys of their names.
    widgets: Vec<(Id, String)>,
}

impl ChooseScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            widgets: env
                .widgets
                .iter()
                .map(|(id, title)| (id.clone(), title.to_string()))
                .collect(),
            list: ListView::new(area, style.row).focus_on_keys(env),
            style,
            header,
        }
    }

    fn toggle(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some((id, _)) = self.widgets.get(index) else {
            return;
        };
        let mut shown = chosen(&env);
        match shown.iter().position(|s| *s == id.as_str()) {
            Some(at) => {
                shown.remove(at);
            }
            None => shown.push(id.as_str()),
        }
        cx.command(Command::SetSetting(
            WIDGETS,
            SettingValue::Text(Cow::Owned(shown.join(", "))),
        ));
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

impl Screen for ChooseScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.widgets.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("home.setting.widgets"),
            None,
            self.list.paging(len),
        );
        if len == 0 {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("home.no-widgets"),
            );
            return;
        }
        let shown = chosen(env);
        let widgets = &self.widgets;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let (id, title) = &widgets[index];
                let at = shown.iter().position(|s| *s == id.as_str());
                let on = at.is_some();
                let title = match at {
                    Some(at) if shown.len() > 1 => format!("{} ({})", env.i18n.tr(title), at + 1),
                    _ => env.i18n.tr(title).to_string(),
                };
                ui::draw_switch_row(canvas, env, &style, rect, focused, &title, "", on);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.widgets.len();
        let rows = self.list.rows() as isize;
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.toggle(self.list.focus, cx);
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
        let len = self.widgets.len();
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
                self.toggle(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}
