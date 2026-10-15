//! Buttons and gestures: what every button, held button, tap zone, held
//! tap and swipe does, changed on the device (Settings › Device).
//!
//! The first row chooses where a change applies: everywhere, or only
//! while reading. Tapping an input lists every action to choose from,
//! *As standard* (what plugins and the button layout bind) and *Nothing*.
//! Changes go to `quireink.ini` like any binding written there
//! (`Command::Bind`) and apply at once.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{GLOBAL_KEYMAP_LAYER, actions, menus, params};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("keys");
/// Opens the list of inputs.
pub const OPEN: Id = Id::new_static("keys.open");
const LIST_SCREEN: Id = Id::new_static("keys.list");
const PICK_SCREEN: Id = Id::new_static("keys.pick");
/// The action picked for an input: parameters `input` and `id` (empty
/// for *As standard*).
const PICKED: Id = Id::new_static("keys.picked");
const INPUT: &str = "input";
/// The reader's screen: bindings that apply while reading.
const READING: &str = "reader.screen";

const BUTTONS: [&str; 10] = [
    "page-forward",
    "page-back",
    "confirm",
    "back",
    "menu",
    "home",
    "left",
    "right",
    "up",
    "down",
];
const ZONES: [&str; 9] = [
    "top-left",
    "top",
    "top-right",
    "left",
    "center",
    "right",
    "bottom-left",
    "bottom",
    "bottom-right",
];
const SWIPES: [&str; 4] = ["left", "right", "up", "down"];

/// The inputs this device has, as in `quireink.ini`.
fn inputs(env: &Env<'_>) -> Vec<String> {
    let mut all = Vec::new();
    if env.device.input.buttons {
        all.extend(BUTTONS.iter().map(|b| format!("button.{b}")));
        all.extend(BUTTONS.iter().map(|b| format!("long.{b}")));
    }
    if env.device.input.touch {
        all.extend(ZONES.iter().map(|z| format!("tap.{z}")));
        all.extend(ZONES.iter().map(|z| format!("long-tap.{z}")));
        all.extend(SWIPES.iter().map(|s| format!("swipe.{s}")));
    }
    all
}

/// An input's name: "Page forward", "Hold: Menu", "Tap: top left".
fn input_name(env: &Env<'_>, input: &str) -> String {
    let (kind, name) = input.split_once('.').unwrap_or((input, ""));
    let part = |prefix: &str| env.i18n.tr(&format!("keys.{prefix}.{name}")).to_string();
    match kind {
        "button" => part("button"),
        "long" => env
            .i18n
            .format("keys.long", &[("input", Arg::Str(&part("button")))]),
        "tap" => env
            .i18n
            .format("keys.tap", &[("zone", Arg::Str(&part("zone")))]),
        "long-tap" => env
            .i18n
            .format("keys.long-tap", &[("zone", Arg::Str(&part("zone")))]),
        "swipe" => part("swipe"),
        _ => input.to_string(),
    }
}

/// The layers an input is looked up in, for everywhere or reading.
fn layers(reading: bool) -> Vec<&'static str> {
    if reading {
        Vec::from([READING, "core.reader", GLOBAL_KEYMAP_LAYER])
    } else {
        Vec::from([GLOBAL_KEYMAP_LAYER])
    }
}

/// The buttons and gestures plugin.
#[derive(Debug, Default)]
pub struct Keys;

impl Plugin for Keys {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Buttons and gestures"),
            description: Cow::Borrowed(
                "What every button, tap zone and swipe does, changed on the device.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: LIST_SCREEN,
            title: Cow::Borrowed("keys.title"),
            factory: Box::new(|env, _| Box::new(ListScreen::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: PICK_SCREEN,
            title: Cow::Borrowed("keys.title"),
            factory: Box::new(|env, params| Box::new(PickScreen::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("keys.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(LIST_SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::SETTINGS,
            item: OPEN,
            order: 75,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("keyboard"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Moves a list's focus by the navigation actions; `false` for others.
fn navigate(list: &mut ListView, len: usize, action: &Id, cx: &mut Cx<'_, '_>) -> bool {
    let rows = list.rows() as isize;
    let a = action;
    let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
        -1
    } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
        1
    } else if *a == actions::NEXT_PAGE {
        rows
    } else if *a == actions::PREV_PAGE {
        -rows
    } else {
        return false;
    };
    if list.move_focus(delta, len) != ListChange::None {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
    true
}

struct ListScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    inputs: Vec<String>,
    /// Changes apply while reading only.
    reading: bool,
}

impl ListScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            inputs: inputs(env),
            reading: false,
            style,
            header,
        }
    }

    /// The place row, then the inputs.
    fn len(&self) -> usize {
        self.inputs.len() + 1
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        if index == 0 {
            self.reading = !self.reading;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return;
        }
        if let Some(input) = self.inputs.get(index - 1) {
            cx.command(Command::OpenScreen(
                PICK_SCREEN,
                Params::new().with(INPUT, input.clone()),
            ));
        }
    }
}

impl Screen for ListScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("keys.title"),
            None,
            self.list.paging(len),
        );
        let (inputs, reading) = (&self.inputs, self.reading);
        let layers = layers(reading);
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                if index == 0 {
                    let place = env.i18n.tr(if reading {
                        "keys.place.reading"
                    } else {
                        "keys.place.everywhere"
                    });
                    let note = env.i18n.tr("keys.place.help");
                    let title = env.i18n.tr("keys.place");
                    ui::draw_noted_row(canvas, env, &style, rect, focused, title, place, note);
                    return;
                }
                let input = &inputs[index - 1];
                let action = env.keys.bound(&layers, input);
                let does = match &action {
                    Some(id) => env
                        .actions
                        .get(id.as_str())
                        .map_or(id.as_str().to_string(), |a| {
                            env.i18n.tr(&a.title).to_string()
                        }),
                    None => env.i18n.tr("core.action.nothing").to_string(),
                };
                let name = input_name(env, input);
                ui::draw_noted_row(canvas, env, &style, rect, focused, &name, "›", &does);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len();
        if navigate(&mut self.list, len, action, cx) {
            return Handled::Yes;
        }
        if *action == actions::CONFIRM {
            self.activate(self.list.focus, cx);
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = self.list.paging(self.len());
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.len()) {
            Some(index) => {
                self.list.focus = index;
                self.activate(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event != PICKED {
            return;
        }
        let Some(input) = params.get(INPUT) else {
            return;
        };
        let action = params.get(params::ID).and_then(|id| Id::parse(id).ok());
        let layer = if self.reading {
            READING
        } else {
            GLOBAL_KEYMAP_LAYER
        };
        cx.command(Command::Bind {
            layer: layer.to_string(),
            input: input.to_string(),
            action,
        });
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

/// Picks what an input does: *As standard*, then every action by name.
struct PickScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    title: String,
    input: String,
    /// Ids (`None`: as standard) and names.
    entries: Vec<(Option<Id>, String)>,
}

impl PickScreen {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let input = params.get(INPUT).unwrap_or_default().to_string();
        let mut actions: Vec<(Option<Id>, String)> = env
            .actions
            .iter()
            .map(|a| (Some(a.id.clone()), env.i18n.tr(&a.title).to_string()))
            .collect();
        actions.sort_by_key(|(_, name)| name.to_lowercase());
        let mut entries = Vec::from([(None, env.i18n.tr("keys.standard").to_string())]);
        entries.extend(actions);
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            title: input_name(env, &input),
            input,
            entries,
            style,
            header,
        }
    }

    fn pick(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some((id, _)) = self.entries.get(index) else {
            return;
        };
        cx.command(Command::Back);
        let mut params = Params::new().with(INPUT, self.input.clone());
        if let Some(id) = id {
            params = params.with(params::ID, id.as_str().to_string());
        }
        cx.command(Command::Emit(PICKED, params));
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
        let len = self.entries.len();
        if navigate(&mut self.list, len, action, cx) {
            return Handled::Yes;
        }
        if *action == actions::CONFIRM {
            self.pick(self.list.focus, cx);
            return Handled::Yes;
        }
        Handled::No
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
