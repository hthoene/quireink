//! The home screen.
//!
//! - **Continue reading**: the last book with its progress, one press away.
//! - **Recently read**: the books before it.
//! - **Widgets**: the ones chosen under Settings › Appearance (a clock,
//!   today's reading, any plugin's), below the card.
//! - **Shortcuts**: the entries of the `home.actions` menu (library and
//!   settings by default). Any plugin can add shortcuts with
//!   `Registrar::add_menu_entry`; users can redefine them in `[menus]`.
//!
//! The recently read list is built from the standard reading events
//! (`core.book-opened`, `core.page-turned`), so it works with any reader
//! plugin. The home screen is bound to `core.home`; replace it with any other
//! screen by configuration or with a plugin of higher priority.

#![no_std]

extern crate alloc;

mod recent;
mod widgets;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params, slots};
use quireink_sdk::document::ImageRef;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, MenuEntry,
    Params, Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, Version,
};

use crate::recent::Recent;

pub const PLUGIN: Id = Id::new_static("home");
pub const SCREEN: Id = Id::new_static("home.dashboard");
/// Shortcuts shown on the home screen.
pub const ACTIONS: Id = Id::new_static("home.actions");

/// The home screen plugin.
#[derive(Debug, Default)]
pub struct Home;

impl Plugin for Home {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Home screen"),
            description: Cow::Borrowed(
                "The start page: continue reading, recent books and shortcuts.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("home.title"),
            factory: Box::new(|env, _| Box::new(Dashboard::new(env))),
        })?;
        r.bind_slot(slots::HOME, SCREEN, Priority::DEFAULT)?;
        for (action, order) in [(actions::LIBRARY, 10), (actions::SETTINGS, 20)] {
            r.add_menu_entry(MenuEntry {
                menu: ACTIONS,
                item: action,
                order,
            })?;
        }
        for event in [events::BOOK_OPENED, events::PAGE_TURNED] {
            r.subscribe(
                event,
                Box::new(|env, params, _| recent::record(env, params)),
            )?;
            r.subscribe(
                events::BOOK_MOVED,
                Box::new(|env, params, _| recent::moved(env, params)),
            )?;
        }
        widgets::register(r)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// An entry of the list below the "continue reading" card.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Book(Recent),
    /// An action with the translation key of its label.
    Action(Id, String),
}

#[derive(Debug)]
struct Dashboard {
    style: UiStyle,
    header: Rect,
    card: Rect,
    /// "Recently read" label above the list, if there is one.
    label: Option<Rect>,
    list: ListView,
    current: Option<Recent>,
    items: Vec<Item>,
    /// The focus is on the card rather than the list.
    on_card: bool,
    /// The current book's cover, once looked up: `(path, cover)`.
    cover: Option<(String, Option<ImageRef>)>,
    /// The widgets shown, where they are, and what they showed when drawn.
    widgets: Vec<(String, Rect, Option<String>)>,
}

impl Dashboard {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let mut dashboard = Self {
            header: Rect::default(),
            card: Rect::default(),
            label: None,
            list: ListView::new(Rect::default(), style.row).focus_on_keys(env),
            style,
            current: None,
            items: Vec::new(),
            on_card: false,
            cover: None,
            widgets: Vec::new(),
        };
        dashboard.load(env);
        dashboard
    }

    /// Reads the recent books and shortcuts and lays out the screen.
    fn load(&mut self, env: &Env<'_>) {
        let style = UiStyle::new(env);
        let mut books = recent::load(env.storage);
        books.retain(|b| env.storage.size(&b.path).is_ok());
        let current = (!books.is_empty()).then(|| books.remove(0));
        let mut items: Vec<Item> = books.into_iter().map(Item::Book).collect();
        let recent_count = items.len();
        items.extend(
            env.menus
                .items(ACTIONS.as_str())
                .into_iter()
                .map(|item| Item::Action(item.id.clone(), String::from(item.title))),
        );

        let display = env.device.display;
        let header = Rect::new(0, 0, display.width, style.header as u16);
        let mut y = header.height + style.gap as u16;
        let margin = style.margin as u16;
        let card = Rect::new(
            margin,
            y,
            display.width.saturating_sub(2 * margin),
            (style.row as f32 * 2.6) as u16,
        );
        y += card.height + style.gap as u16;
        // The widgets chosen, between the margins.
        let width = display.width.saturating_sub(2 * margin);
        self.widgets = widgets::chosen(env)
            .into_iter()
            .filter_map(|id| {
                let height = env.widgets.height(id, env, width)?;
                let rect = Rect::new(margin, y, width, height);
                y += height + style.gap as u16;
                Some((id.to_string(), rect, env.widgets.signature(id, env)))
            })
            .collect();
        let label = (recent_count > 0).then(|| {
            let rect = Rect::new(0, y, display.width, (style.row as f32 * 0.7) as u16);
            y += rect.height;
            rect
        });
        let area = Rect::new(0, y, display.width, display.height.saturating_sub(y));

        let focus = self.list.focus.min(items.len().saturating_sub(1));
        self.list = ListView::new(area, style.row).focus_on_keys(env);
        self.list.focus = focus;
        self.on_card = current.is_some() && (self.on_card || self.current.is_none());
        self.list.active = !self.on_card;
        self.style = style;
        self.header = header;
        self.card = card;
        self.label = label;
        self.current = current;
        self.items = items;
    }

    fn focus_card(&mut self, on_card: bool, cx: &mut Cx<'_, '_>) {
        let on_card = on_card && self.current.is_some();
        if on_card != self.on_card {
            self.on_card = on_card;
            self.list.active = !on_card;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn moved(change: ListChange, cx: &mut Cx<'_, '_>) {
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
    }

    fn up(&mut self, cx: &mut Cx<'_, '_>) {
        if self.on_card {
            return;
        }
        if self.list.focus == 0 {
            self.focus_card(true, cx);
        } else {
            Self::moved(self.list.move_focus(-1, self.items.len()), cx);
        }
    }

    fn down(&mut self, cx: &mut Cx<'_, '_>) {
        if self.on_card {
            if !self.items.is_empty() {
                self.list.focus = 0;
                self.focus_card(false, cx);
            }
            return;
        }
        Self::moved(self.list.move_focus(1, self.items.len()), cx);
    }

    fn open_book(path: &str, cx: &mut Cx<'_, '_>) {
        cx.command(Command::OpenSlot(
            slots::READER,
            Params::new().with(params::PATH, path),
        ));
    }

    fn open(&self, cx: &mut Cx<'_, '_>) {
        if self.on_card {
            if let Some(book) = &self.current {
                Self::open_book(&book.path, cx);
            }
            return;
        }
        match self.items.get(self.list.focus) {
            Some(Item::Book(book)) => Self::open_book(&book.path, cx),
            Some(Item::Action(action, _)) => cx.command(Command::RunAction(action.clone())),
            None => {}
        }
    }

    fn draw_card(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.card;
        let Some(font) = style.font(env) else {
            return;
        };
        // A thick frame shows the focus, once buttons are used.
        let border = if self.on_card && self.list.shown {
            style.line * 5
        } else {
            style.line * 2
        };
        canvas.stroke_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
            border,
            Color::BLACK,
        );
        let gray = Color::gray(90);
        let Some(book) = &self.current else {
            ui::draw_message(canvas, env, &style, r, env.i18n.tr("home.nothing-read"));
            return;
        };

        // The cover on the left, if the book has one.
        let inset = style.margin as u16 + border as u16;
        let cover = self
            .cover
            .as_ref()
            .filter(|(path, _)| *path == book.path)
            .and_then(|(_, cover)| cover.as_ref());
        let r = match cover {
            Some(cover) => {
                let (w, h) = ui::fit_image(cover, r.width / 4, r.height.saturating_sub(2 * inset));
                let area = Rect::new(r.x + inset, r.y + (r.height - h) / 2, w, h);
                ui::draw_image(canvas, env.storage, cover, area);
                let shift = inset + w;
                Rect::new(r.x + shift, r.y, r.width.saturating_sub(shift), r.height)
            }
            None => r,
        };
        let pad = style.margin as f32 + border as f32;
        let inner_w = f32::from(r.width) - 2.0 * pad;
        let third = r.height / 3;
        let line = |i: u16| Rect::new(r.x, r.y + i * third, r.width, third);
        ui::draw_label(
            canvas,
            &font,
            style.small,
            f32::from(r.x) + pad,
            line(0),
            env.i18n.tr("home.continue"),
            gray,
        );
        let percent = env.i18n.format(
            "home.percent",
            &[("percent", Arg::Int((book.progress + 0.5) as i64))],
        );
        let pw = font.measure(style.small, &percent);
        ui::draw_label(
            canvas,
            &font,
            style.small,
            f32::from(r.x) + f32::from(r.width) - pad - pw,
            line(0),
            &percent,
            gray,
        );
        // Long titles in the body size rather than cut short.
        let size = if font.measure(style.title, &book.title) <= inner_w {
            style.title
        } else {
            style.text
        };
        let title = ui::fit(&font, size, &book.title, inner_w);
        ui::draw_label(
            canvas,
            &font,
            size,
            f32::from(r.x) + pad,
            line(1),
            &title,
            Color::BLACK,
        );
        // Progress bar.
        let bar_h = (style.line * 4).max(3);
        let bar = line(2);
        let x = i32::from(r.x) + pad as i32;
        let y = i32::from(bar.y) + i32::from(bar.height) / 2 - bar_h as i32 / 2;
        let w = inner_w.max(1.0) as u32;
        canvas.stroke_rect(x, y, w, bar_h, style.line.max(1), Color::BLACK);
        let filled = (w as f32 * (book.progress / 100.0).clamp(0.0, 1.0)) as u32;
        canvas.fill_rect(x, y, filled, bar_h, Color::BLACK);
    }
}

impl Screen for Dashboard {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        ui::draw_header(
            canvas,
            env,
            &style,
            self.header,
            env.i18n.tr("home.title"),
            None,
        );
        self.draw_card(canvas, env);
        for (id, rect, _) in &self.widgets {
            ui::fill(canvas, *rect, Color::WHITE);
            env.widgets.draw(id, canvas, env, *rect);
        }
        let Some(font) = style.font(env) else {
            return;
        };
        if let Some(label) = self.label {
            ui::draw_label(
                canvas,
                &font,
                style.small,
                style.margin as f32,
                label,
                env.i18n.tr("home.recent"),
                Color::gray(90),
            );
        }
        // Like menu entries: an icon, the label, and what is behind it.
        let items = &self.items;
        let room = ui::icon_room(env, &style, true);
        self.list.draw(
            canvas,
            &style,
            items.len(),
            |canvas, index, rect, focused| {
                let ink = if focused { Color::WHITE } else { Color::BLACK };
                match &items[index] {
                    Item::Book(book) => {
                        let rect =
                            ui::draw_row_icon(canvas, env, &style, rect, room, Some("book"), ink);
                        let percent = env.i18n.format(
                            "home.percent",
                            &[("percent", Arg::Int((book.progress + 0.5) as i64))],
                        );
                        ui::draw_value_row(
                            canvas,
                            env,
                            &style,
                            rect,
                            focused,
                            &book.title,
                            &percent,
                        );
                    }
                    Item::Action(id, title) => {
                        let icon = env.menus.icon(id.as_str());
                        let rect = ui::draw_row_icon(canvas, env, &style, rect, room, icon, ink);
                        let title = env.i18n.tr(title);
                        ui::draw_value_row(canvas, env, &style, rect, focused, title, "›");
                    }
                }
            },
        );
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        // The reader may have changed the recent books meanwhile.
        let env = *cx.env();
        self.load(&env);
        cx.wake_at(env.now);
    }

    /// Looks up the current book's cover (opening the book through its
    /// format plugin, which caches the cover).
    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        // Widgets whose content changed (a clock each minute).
        let mut changed = false;
        for (id, rect, shown) in &mut self.widgets {
            let now = env.widgets.signature(id, &env);
            if now.is_some() && now != *shown {
                *shown = now;
                cx.request_redraw_area(*rect);
                changed = true;
            }
        }
        if changed {
            cx.request_refresh(RefreshMode::Fast);
        }
        if self.widgets.iter().any(|(_, _, s)| s.is_some()) {
            let next = quireink_sdk::time::next_minute(&env).unwrap_or(env.now.add_millis(60_000));
            cx.wake_at(next);
        }
        let Some(book) = &self.current else {
            return;
        };
        if self
            .cover
            .as_ref()
            .is_some_and(|(path, _)| *path == book.path)
        {
            return;
        }
        let cover = env
            .formats
            .open(env.storage, &book.path)
            .ok()
            .and_then(|document| document.cover(env.storage).ok().flatten());
        let found = cover.is_some();
        self.cover = Some((book.path.clone(), cover));
        if found {
            cx.request_redraw_area(self.card);
            cx.request_refresh(RefreshMode::Partial);
        }
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::SETTING_CHANGED {
            let env = *cx.env();
            self.load(&env);
            cx.request_redraw();
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let rows = self.list.rows() as isize;
        let navigation = [
            actions::UP,
            actions::LEFT,
            actions::PREVIOUS,
            actions::DOWN,
            actions::RIGHT,
            actions::NEXT,
            actions::NEXT_PAGE,
            actions::PREV_PAGE,
        ];
        if navigation.contains(a) && !self.list.shown {
            // The first button press shows where the focus is.
            self.list.shown = true;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
            return Handled::Yes;
        }
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            self.up(cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            self.down(cx);
        } else if *a == actions::NEXT_PAGE {
            if self.on_card {
                self.down(cx);
            } else {
                Self::moved(self.list.move_focus(rows, self.items.len()), cx);
            }
        } else if *a == actions::PREV_PAGE {
            if self.list.focus == 0 {
                self.focus_card(true, cx);
            } else {
                Self::moved(self.list.move_focus(-rows, self.items.len()), cx);
            }
        } else if *a == actions::CONFIRM {
            self.open(cx);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if self.card.contains(gesture.start) {
            if let Some(book) = &self.current {
                Self::open_book(&book.path.clone(), cx);
                return Handled::Yes;
            }
            return Handled::No;
        }
        match self.list.hit(gesture.start, self.items.len()) {
            Some(index) => {
                self.list.focus = index;
                self.on_card = false;
                self.list.active = true;
                self.open(cx);
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
