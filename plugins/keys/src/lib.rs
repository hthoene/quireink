//! Buttons and gestures: what every swipe in from an edge, swipe, tap
//! zone, held tap, button and held button does, changed on the device
//! (Settings › Device).
//!
//! The first row chooses where a change applies: everywhere, on the home
//! screen, while reading, in the library, or in one tool. For a place
//! other than everywhere the second row says how it takes gestures: as
//! standard, its own (gestures go to the tool, a swipe in from an edge
//! needs a second one: for games and drawing), or none. A tool sets its
//! own default with `gestures.policy` in its key map; the user's choice
//! wins.
//!
//! The inputs follow in sections. Tapping one lists the actions to choose
//! from, in groups and searchable, after *As standard* (what plugins and
//! the button layout bind) and *Nothing*. Changes go to `quireink.ini`
//! like any binding written there (`Command::Bind`) and apply at once.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{GLOBAL_KEYMAP_LAYER, actions, menus, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, Rows, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, Version,
};

pub const PLUGIN: Id = Id::new_static("keys");
/// Opens the list of inputs.
pub const OPEN: Id = Id::new_static("keys.open");
const LIST_SCREEN: Id = Id::new_static("keys.list");
const PICK_SCREEN: Id = Id::new_static("keys.pick");
const PLACE_SCREEN: Id = Id::new_static("keys.place");
/// The action picked for an input: parameters `input` and `id` (empty
/// for *As standard*).
const PICKED: Id = Id::new_static("keys.picked");
/// A place was chosen: parameter `id`, its index.
const PLACED: Id = Id::new_static("keys.placed");
/// The search words typed: parameter `text`.
const SEARCHED: Id = Id::new_static("keys.searched");
const INPUT: &str = "input";
/// How a place takes gestures, as written in key maps.
const POLICY: &str = "gestures.policy";
const POLICIES: [&str; 3] = [
    "core.gestures.standard",
    "core.gestures.own",
    "core.gestures.off",
];
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
const EDGES: [&str; 4] = ["bottom", "top", "left", "right"];

/// Where a change applies: the key map layers looked up there, and the
/// one changes go to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    /// Translation key, or a screen's name.
    title: String,
    layers: Vec<String>,
}

impl Place {
    fn layer(&self) -> &str {
        &self.layers[0]
    }

    fn layer_refs(&self) -> Vec<&str> {
        self.layers.iter().map(String::as_str).collect()
    }

    fn everywhere(&self) -> bool {
        self.layer() == GLOBAL_KEYMAP_LAYER
    }
}

/// The places changes may apply: everywhere, the home screen, reading,
/// the library, then the tools that take gestures their own way.
fn places(env: &Env<'_>) -> Vec<Place> {
    let place = |title: &str, layers: &[&str]| Place {
        title: env.i18n.tr(title).to_string(),
        layers: layers.iter().map(|l| l.to_string()).collect(),
    };
    let mut places = Vec::from([
        place("keys.place.everywhere", &[GLOBAL_KEYMAP_LAYER]),
        place(
            "keys.place.home",
            &[slots::HOME.as_str(), GLOBAL_KEYMAP_LAYER],
        ),
        place(
            "keys.place.reading",
            &[READING, slots::READER.as_str(), GLOBAL_KEYMAP_LAYER],
        ),
        place(
            "keys.place.library",
            &[slots::LIBRARY.as_str(), GLOBAL_KEYMAP_LAYER],
        ),
    ]);
    // Tools that say how they take gestures.
    let mut tools: Vec<Place> = env
        .screens
        .iter()
        .filter(|(id, _)| {
            id.as_str() != READING && env.keys.bound(&[id.as_str()], POLICY).is_some()
        })
        .map(|(id, title)| Place {
            title: env.i18n.tr(title).to_string(),
            layers: Vec::from([id.as_str().to_string(), GLOBAL_KEYMAP_LAYER.to_string()]),
        })
        .collect();
    tools.sort_by_key(|p| p.title.to_lowercase());
    places.extend(tools);
    places
}

/// Every screen as a place, for choosing one not listed.
fn all_screens(env: &Env<'_>) -> Vec<Place> {
    let mut all: Vec<Place> = env
        .screens
        .iter()
        .map(|(id, title)| Place {
            title: env.i18n.tr(title).to_string(),
            layers: Vec::from([id.as_str().to_string(), GLOBAL_KEYMAP_LAYER.to_string()]),
        })
        .collect();
    all.sort_by_key(|p| p.title.to_lowercase());
    all.dedup_by(|a, b| a.title == b.title);
    all
}

/// A row of the list of inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Place,
    Policy,
    Heading(&'static str),
    Input(String),
}

/// The rows for this device: the place (and its gestures), then the
/// inputs in sections, gestures first on touch screens.
fn rows(env: &Env<'_>, everywhere: bool) -> Vec<Row> {
    let input = env.device.input;
    let mut rows = Vec::from([Row::Place]);
    if !everywhere && input.touch {
        rows.push(Row::Policy);
    }
    let section = |rows: &mut Vec<Row>, heading: &'static str, inputs: Vec<String>| {
        if !inputs.is_empty() {
            rows.push(Row::Heading(heading));
            rows.extend(inputs.into_iter().map(Row::Input));
        }
    };
    let mut gestures: Vec<(&'static str, Vec<String>)> = Vec::new();
    if input.touch {
        let each = |prefix: &str, names: &[&str]| -> Vec<String> {
            names.iter().map(|n| format!("{prefix}.{n}")).collect()
        };
        gestures.push(("keys.section.edges", each("edge", &EDGES)));
        gestures.push(("keys.section.swipes", each("swipe", &SWIPES)));
        gestures.push(("keys.section.taps", each("tap", &ZONES)));
        gestures.push(("keys.section.held-taps", each("long-tap", &ZONES)));
    }
    let mut buttons: Vec<(&'static str, Vec<String>)> = Vec::new();
    if input.buttons {
        let has: Vec<&str> = BUTTONS
            .iter()
            .copied()
            .filter(|b| input.keys.is_empty() || input.keys.contains(b))
            .collect();
        buttons.push((
            "keys.section.buttons",
            has.iter().map(|b| format!("button.{b}")).collect(),
        ));
        buttons.push((
            "keys.section.held-buttons",
            has.iter().map(|b| format!("long.{b}")).collect(),
        ));
    }
    let (first, then) = if input.touch {
        (gestures, buttons)
    } else {
        (buttons, gestures)
    };
    for (heading, inputs) in first.into_iter().chain(then) {
        section(&mut rows, heading, inputs);
    }
    rows
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
        "edge" => part("edge"),
        _ => input.to_string(),
    }
}

/// The name of action `id`.
fn action_name(env: &Env<'_>, id: &Id) -> String {
    env.actions
        .get(id.as_str())
        .map_or(id.as_str().to_string(), |a| {
            env.i18n.tr(&a.title).to_string()
        })
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
        r.add_screen(ScreenSpec {
            id: PLACE_SCREEN,
            title: Cow::Borrowed("keys.place"),
            factory: Box::new(|env, params| Box::new(PlaceScreen::new(env, params))),
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

/// Draws a small 3×3 grid at the left of `rect` with `zone` filled: the
/// part of the screen a tap zone is.
fn draw_zone(canvas: &mut Canvas<'_>, style: &UiStyle, rect: Rect, zone: &str, ink: Color) {
    let Some(index) = ZONES.iter().position(|z| *z == zone) else {
        return;
    };
    let cell = (rect.height / 6).max(4);
    let side = cell * 3;
    let x = rect.x + style.margin as u16;
    let y = rect.y + (rect.height - side) / 2;
    for i in 0..9u16 {
        let (cx, cy) = (x + (i % 3) * cell, y + (i / 3) * cell);
        if usize::from(i) == index {
            canvas.fill_rect(
                i32::from(cx),
                i32::from(cy),
                u32::from(cell),
                u32::from(cell),
                ink,
            );
        } else {
            canvas.stroke_rect(
                i32::from(cx),
                i32::from(cy),
                u32::from(cell),
                u32::from(cell),
                1,
                ink,
            );
        }
    }
}

struct ListScreen {
    style: UiStyle,
    header: ui::Header,
    rows: Rows,
    entries: Vec<Row>,
    places: Vec<Place>,
    place: usize,
}

impl ListScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let mut screen = Self {
            rows: Rows::new(Rect::default(), Vec::new(), Vec::new()),
            entries: Vec::new(),
            places: places(env),
            place: 0,
            style,
            header,
        };
        screen.build(env);
        screen
    }

    /// The rows for the place chosen.
    fn build(&mut self, env: &Env<'_>) {
        let display = env.device.display;
        let top = self.header.area.height + self.style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let focus = self.rows.focus;
        self.entries = rows(env, self.place().everywhere());
        let heights = self
            .entries
            .iter()
            .map(|row| match row {
                Row::Heading(_) => ui::heading_height(env, &self.style),
                _ => self.style.noted_row() as u16,
            })
            .collect();
        let headings = self
            .entries
            .iter()
            .map(|row| matches!(row, Row::Heading(_)))
            .collect();
        self.rows = Rows::new(area, heights, headings);
        self.rows.focus = focus.min(self.entries.len().saturating_sub(1));
    }

    fn place(&self) -> &Place {
        &self.places[self.place.min(self.places.len() - 1)]
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match self.entries.get(index) {
            Some(Row::Place) => {
                cx.command(Command::OpenScreen(PLACE_SCREEN, Params::new()));
            }
            Some(Row::Policy) => {
                // Standard, own, off, and round again.
                let place = self.place().clone();
                let now = env.keys.bound(&place.layer_refs(), POLICY);
                let at = now
                    .as_ref()
                    .and_then(|id| POLICIES.iter().position(|p| *p == id.as_str()))
                    .unwrap_or(0);
                let next = Id::parse(POLICIES[(at + 1) % POLICIES.len()]).ok();
                cx.command(Command::Bind {
                    layer: place.layer().to_string(),
                    input: POLICY.to_string(),
                    action: next,
                });
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
            Some(Row::Input(input)) => {
                cx.command(Command::OpenScreen(
                    PICK_SCREEN,
                    Params::new().with(INPUT, input.clone()),
                ));
            }
            _ => {}
        }
    }
}

impl Screen for ListScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("keys.title"),
            None,
            Some(self.rows.paging()),
        );
        let place = self.place().clone();
        let layers = place.layer_refs();
        let entries = &self.entries;
        let room = ui::icon_room(env, &style, true);
        let shown = !env.device.input.touch;
        self.rows
            .draw(canvas, &style, shown, |canvas, index, rect, focused| {
                let ink = if focused { Color::WHITE } else { Color::BLACK };
                match &entries[index] {
                    Row::Heading(key) => {
                        ui::draw_heading(canvas, env, &style, rect, env.i18n.tr(key))
                    }
                    Row::Place => {
                        let title = env.i18n.tr("keys.place");
                        let note = env.i18n.tr("keys.place.help");
                        ui::draw_noted_row(
                            canvas,
                            env,
                            &style,
                            rect,
                            focused,
                            title,
                            &place.title,
                            note,
                        );
                    }
                    Row::Policy => {
                        let now = env.keys.bound(&layers, POLICY);
                        let value = now
                            .as_ref()
                            .filter(|id| POLICIES.contains(&id.as_str()))
                            .map_or(POLICIES[0], Id::as_str);
                        let title = env.i18n.tr("keys.policy");
                        let (help, name) = (format!("keys.{value}.help"), format!("keys.{value}"));
                        let (note, shown) = (env.i18n.tr(&help), env.i18n.tr(&name));
                        ui::draw_noted_row(canvas, env, &style, rect, focused, title, shown, note);
                    }
                    Row::Input(input) => {
                        let action = env.keys.bound(&layers, input);
                        let does = match &action {
                            Some(id) => action_name(env, id),
                            None => env.i18n.tr("core.action.nothing").to_string(),
                        };
                        // Tap zones as a small picture of the screen.
                        let zone = input
                            .strip_prefix("tap.")
                            .or_else(|| input.strip_prefix("long-tap."));
                        let rect = match zone {
                            Some(zone) => {
                                let band = ui::title_area(&style, rect);
                                draw_zone(canvas, &style, band, zone, ink);
                                let shift = room.min(rect.width);
                                Rect::new(rect.x + shift, rect.y, rect.width - shift, rect.height)
                            }
                            None => rect,
                        };
                        let name = input_name(env, input);
                        ui::draw_noted_row(canvas, env, &style, rect, focused, &name, "›", &does);
                    }
                }
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE || *a == actions::PREV_PAGE {
            let change = self.rows.turn(*a == actions::NEXT_PAGE);
            if change != ListChange::None {
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
            return Handled::Yes;
        } else if *a == actions::CONFIRM {
            self.activate(self.rows.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.rows.step(delta > 0) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = Some(self.rows.paging());
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.rows.hit(gesture.start) {
            Some(index) => {
                self.rows.focus = index;
                self.activate(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if *event == PLACED {
            let Some(chosen) = params.get(params::ID) else {
                return;
            };
            // A place listed, or any screen chosen from all of them.
            if let Some(index) = self.places.iter().position(|p| p.layer() == chosen) {
                self.place = index;
            } else if let Some(place) = all_screens(&env).into_iter().find(|p| p.layer() == chosen)
            {
                self.places.push(place);
                self.place = self.places.len() - 1;
            }
            self.build(&env);
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Partial);
            return;
        }
        if *event != PICKED {
            return;
        }
        let Some(input) = params.get(INPUT) else {
            return;
        };
        let action = params.get(params::ID).and_then(|id| Id::parse(id).ok());
        cx.command(Command::Bind {
            layer: self.place().layer().to_string(),
            input: input.to_string(),
            action,
        });
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }
}

/// Chooses where changes apply: the places, then every other screen.
struct PlaceScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    places: Vec<Place>,
}

impl PlaceScreen {
    fn new(env: &Env<'_>, _: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let mut places = places(env);
        for screen in all_screens(env) {
            if !places.iter().any(|p| p.layer() == screen.layer()) {
                places.push(screen);
            }
        }
        Self {
            list: ListView::new(area, style.row).focus_on_keys(env),
            places,
            style,
            header,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some(place) = self.places.get(index) {
            cx.command(Command::Back);
            cx.command(Command::Emit(
                PLACED,
                Params::new().with(params::ID, place.layer().to_string()),
            ));
        }
    }
}

impl Screen for PlaceScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.places.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("keys.place"),
            None,
            self.list.paging(len),
        );
        let places = &self.places;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                ui::draw_value_row(canvas, env, &style, rect, focused, &places[index].title, "");
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.places.len();
        if navigate(&mut self.list, len, action, cx) {
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
        let len = self.places.len();
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
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}

/// The groups actions are listed in.
const GROUPS: [&str; 5] = [
    "keys.group.navigation",
    "keys.group.reading",
    "keys.group.tools",
    "keys.group.system",
    "keys.group.more",
];

/// Moving around and turning pages: the steps buttons and gestures take.
const NAVIGATION: [&str; 12] = [
    "core.next-page",
    "core.prev-page",
    "core.next",
    "core.previous",
    "core.up",
    "core.down",
    "core.left",
    "core.right",
    "core.confirm",
    "core.back",
    "core.home",
    "core.menu",
];

/// The group of action `id` (an index into [`GROUPS`]): navigation, what
/// the reader's menus hold or reader actions, tools in the tools menus,
/// the kernel's own, the rest.
fn group_of(env: &Env<'_>, id: &Id) -> usize {
    let name = id.as_str();
    if NAVIGATION.contains(&name) {
        return 0;
    }
    let in_menu = |menu: &str| env.menus.items(menu).iter().any(|item| item.id == id);
    if name.starts_with("reader.") || in_menu("reader.menu") || in_menu("reader.more") {
        return 1;
    }
    if in_menu(menus::TOOLS.as_str()) || in_menu("home.actions") {
        return 2;
    }
    if name.starts_with("core.") {
        return 3;
    }
    4
}

/// A row of the action picker.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Search,
    Standard,
    Action(Id, String),
    Heading(&'static str),
}

/// Picks what an input does: a search, *As standard* and *Nothing*, then
/// every action by group and name (or those the search finds).
struct PickScreen {
    style: UiStyle,
    header: ui::Header,
    rows: Rows,
    title: String,
    input: String,
    entries: Vec<Pick>,
    query: String,
}

impl PickScreen {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let input = params.get(INPUT).unwrap_or_default().to_string();
        let mut screen = Self {
            rows: Rows::new(Rect::default(), Vec::new(), Vec::new()),
            title: input_name(env, &input),
            input,
            entries: Vec::new(),
            query: String::new(),
            style,
            header,
        };
        screen.build(env);
        screen
    }

    fn build(&mut self, env: &Env<'_>) {
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        // Nothing, standard and the gesture policies are no actions to pick.
        let hidden = |id: &Id| *id == actions::NOTHING || id.as_str().starts_with("core.gestures.");
        let mut found: Vec<(usize, Id, String)> = env
            .actions
            .iter()
            .filter(|a| !hidden(&a.id))
            .map(|a| {
                (
                    group_of(env, &a.id),
                    a.id.clone(),
                    env.i18n.tr(&a.title).to_string(),
                )
            })
            .filter(|(_, _, name)| {
                let name = name.to_lowercase();
                words.iter().all(|w| name.contains(w.as_str()))
            })
            .collect();
        found.sort_by_key(|(group, _, name)| (*group, name.to_lowercase()));
        let mut entries = Vec::from([Pick::Search]);
        if words.is_empty() {
            entries.push(Pick::Standard);
            entries.push(Pick::Action(
                actions::NOTHING,
                env.i18n.tr("core.action.nothing").to_string(),
            ));
        }
        let mut group = None;
        for (g, id, name) in found {
            if group != Some(g) {
                entries.push(Pick::Heading(GROUPS[g]));
                group = Some(g);
            }
            entries.push(Pick::Action(id, name));
        }
        let display = env.device.display;
        let top = self.header.area.height + self.style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let heights = entries
            .iter()
            .map(|e| match e {
                Pick::Heading(_) => ui::heading_height(env, &self.style),
                _ => self.style.row as u16,
            })
            .collect();
        let headings = entries
            .iter()
            .map(|e| matches!(e, Pick::Heading(_)))
            .collect();
        self.rows = Rows::new(area, heights, headings);
        self.entries = entries;
    }

    fn pick(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let id = match self.entries.get(index) {
            Some(Pick::Search) => {
                let env = *cx.env();
                cx.command(Command::OpenSlot(
                    slots::KEYBOARD,
                    Params::new()
                        .with(params::TITLE, env.i18n.tr("keys.search").to_string())
                        .with(params::TEXT, self.query.clone())
                        .with(params::REPLY, SEARCHED.as_str().to_string()),
                ));
                return;
            }
            Some(Pick::Standard) => None,
            Some(Pick::Action(id, _)) => Some(id.clone()),
            _ => return,
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
        self.header.draw(
            canvas,
            env,
            &style,
            &self.title,
            None,
            Some(self.rows.paging()),
        );
        let entries = &self.entries;
        let query = &self.query;
        let room = ui::icon_room(env, &style, true);
        let shown = !env.device.input.touch;
        self.rows
            .draw(canvas, &style, shown, |canvas, index, rect, focused| {
                let ink = if focused { Color::WHITE } else { Color::BLACK };
                match &entries[index] {
                    Pick::Heading(key) => {
                        ui::draw_heading(canvas, env, &style, rect, env.i18n.tr(key))
                    }
                    Pick::Search => {
                        let rect =
                            ui::draw_row_icon(canvas, env, &style, rect, room, Some("search"), ink);
                        let title = if query.is_empty() {
                            env.i18n.tr("keys.search")
                        } else {
                            query.as_str()
                        };
                        ui::draw_value_row(canvas, env, &style, rect, focused, title, "");
                    }
                    Pick::Standard => {
                        let rect = ui::draw_row_icon(canvas, env, &style, rect, room, None, ink);
                        let title = env.i18n.tr("keys.standard");
                        ui::draw_value_row(canvas, env, &style, rect, focused, title, "");
                    }
                    Pick::Action(id, name) => {
                        let icon = env.menus.icon(id.as_str());
                        let rect = ui::draw_row_icon(canvas, env, &style, rect, room, icon, ink);
                        ui::draw_value_row(canvas, env, &style, rect, focused, name, "");
                    }
                }
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE || *a == actions::PREV_PAGE {
            if self.rows.turn(*a == actions::NEXT_PAGE) != ListChange::None {
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
            return Handled::Yes;
        } else if *a == actions::CONFIRM {
            self.pick(self.rows.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if self.rows.step(delta > 0) != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = Some(self.rows.paging());
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.rows.hit(gesture.start) {
            Some(index) => {
                self.rows.focus = index;
                self.pick(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == SEARCHED {
            self.query = params
                .get(params::TEXT)
                .unwrap_or_default()
                .trim()
                .to_string();
            let env = *cx.env();
            self.build(&env);
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Partial);
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
