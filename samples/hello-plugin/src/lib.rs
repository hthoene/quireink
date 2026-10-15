//! A sample QuireInk plugin to start from: everything a plugin usually
//! needs, small. See `docs/plugins.md` for the walk through it.
//!
//! - an **action**, *Hello*, listed in the tools menu with an icon;
//! - a **screen** it opens, usable by touch and by buttons alike;
//! - a **setting**, the name to greet, listed in the settings;
//! - an **event** it listens to: books opened, counted;
//! - **translations**, English and German.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::Cell;

use quireink_sdk::builtin::{actions, events, groups, menus};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Version,
};

/// Ids start with the plugin's id: `hello.…`.
pub const PLUGIN: Id = Id::new_static("hello");
pub const OPEN: Id = Id::new_static("hello.open");
pub const SCREEN: Id = Id::new_static("hello.screen");
pub const NAME: Id = Id::new_static("hello.name");

/// The plugin. State shared between its parts lives in `Rc`s it hands to
/// them when it registers.
#[derive(Debug, Default)]
pub struct Hello;

impl Plugin for Hello {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Hello"),
            description: Cow::Borrowed("A sample plugin to start from."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        // Books opened since the start, counted by the event below and
        // shown by the screen.
        let opened = Rc::new(Cell::new(0u32));

        // The action: what a menu entry, a button or a gesture runs.
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("hello.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 90,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("star"))?;

        // The screen, made each time it opens.
        let count = opened.clone();
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("hello.title"),
            factory: Box::new(move |env, _| Box::new(HelloScreen::new(env, count.get()))),
        })?;

        // The setting: shown in the settings, saved by the kernel.
        r.add_setting(
            SettingSpec::new(
                NAME,
                "hello.setting.name",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("")),
            )
            .group(groups::OTHER)
            .description("hello.setting.name.help"),
        )?;

        // The event: every reader emits it when a book opens.
        r.subscribe(
            events::BOOK_OPENED,
            Box::new(move |_: &Env<'_>, _: &Params, _: &mut Vec<Command>| {
                opened.set(opened.get() + 1);
            }),
        )?;

        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// The greeting, under a header with a back arrow.
struct HelloScreen {
    style: UiStyle,
    header: ui::Header,
    opened: u32,
}

impl HelloScreen {
    fn new(env: &Env<'_>, opened: u32) -> Self {
        let style = UiStyle::new(env);
        Self {
            header: ui::Header::new(env, &style, true),
            style,
            opened,
        }
    }

    fn greeting(&self, env: &Env<'_>) -> alloc::string::String {
        let name = match env.settings.text(NAME.as_str()).trim() {
            "" => env.i18n.tr("hello.someone"),
            name => name,
        };
        let count = i64::from(self.opened);
        env.i18n.plural(
            "hello.greeting",
            count,
            &[("name", Arg::Str(name)), ("count", Arg::Int(count))],
        )
    }
}

impl Screen for HelloScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let title = env.i18n.tr("hello.title");
        self.header
            .draw(canvas, env, &self.style, title, None, None);
        let display = env.device.display;
        let top = self.header.area.height;
        let area = Rect::new(0, top, display.width, display.height - top);
        ui::draw_message(canvas, env, &self.style, area, &self.greeting(env));
    }

    /// Buttons: *confirm* or *back* close it; other actions (e.g. the
    /// menu button) go on to their global handlers.
    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if *action == actions::CONFIRM || *action == actions::BACK {
            cx.command(Command::Back);
            return Handled::Yes;
        }
        Handled::No
    }

    /// Touch: the header's back arrow, or a tap anywhere else, closes it.
    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(action) = self.header.tap(cx.env(), &self.style, gesture.start, None) {
            cx.command(Command::RunAction(action));
        } else {
            cx.command(Command::Back);
        }
        Handled::Yes
    }
}
