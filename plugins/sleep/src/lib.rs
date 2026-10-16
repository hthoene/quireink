//! The sleep screen (slot `core.sleep`): what the screen shows while the
//! device sleeps, e-ink keeping it without power.
//!
//! Setting `sleep.show`:
//!
//! - *Cover* (the default): the cover of the book read last;
//! - *Picture*: the pictures (JPEG, PNG) of a folder (`sleep.folder`,
//!   `sleep` at first, as on CrossPoint), one after the other;
//! - *Message*: just "Sleeping";
//! - *Widgets*: widgets as on the home screen (setting `sleep.widgets`,
//!   edited with the home screen's editor), e.g. the book being read and
//!   today's reading, in the middle of the screen;
//! - *Page*: the screen stays as it is.
//!
//! Without a cover or a picture the message shows. The screen closes when
//! the device wakes. Only where QuireInk shows the sleep screen itself
//! (the X4, not Android): elsewhere its settings are hidden.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{events, groups, params, slots};
use quireink_sdk::document::ImageRef;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Handled, Id, Manifest, MenuNeed, Params, Plugin, Priority,
    Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec, SettingKind,
    SettingSpec, SettingValue, Storage, Version,
};

pub const PLUGIN: Id = Id::new_static("sleep");
/// What the sleep screen shows: one of [`COVER`], [`PICTURE`],
/// [`MESSAGE`], [`PAGE`].
pub const SHOW: Id = Id::new_static("sleep.show");
pub const COVER: &str = "sleep.show.cover";
pub const PICTURE: &str = "sleep.show.picture";
pub const MESSAGE: &str = "sleep.show.message";
pub const PAGE: &str = "sleep.show.page";
pub const WIDGETS: &str = "sleep.show.widgets";
/// The widgets shown with [`WIDGETS`], ids separated by commas.
pub const WIDGET_LIST: Id = Id::new_static("sleep.widgets");
/// The home screen's editor of lists of widgets, if installed.
const EDIT_WIDGETS: Id = Id::new_static("home.edit");
/// The folder of the pictures.
pub const FOLDER: Id = Id::new_static("sleep.folder");
const SCREEN: Id = Id::new_static("sleep.screen");
const SECTION: &str = "sleep.section";
/// The folder chooser of the `files` plugin, if installed.
const CHOOSE_FOLDER: Id = Id::new_static("files.choose-folder");

fn data(name: &str) -> String {
    format!("{}/{name}", Env::plugin_dir(PLUGIN.as_str()))
}

/// The sleep screen plugin.
#[derive(Debug, Default)]
pub struct Sleep;

impl Plugin for Sleep {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Sleep screen"),
            description: Cow::Borrowed(
                "Shows the book's cover, your pictures or a message while the device sleeps.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_setting(
            SettingSpec::new(
                SHOW,
                "sleep.setting.show",
                SettingKind::Choice(
                    [COVER, PICTURE, WIDGETS, MESSAGE, PAGE]
                        .into_iter()
                        .map(Cow::Borrowed)
                        .collect(),
                ),
                SettingValue::Text(Cow::Borrowed(COVER)),
            )
            .group(groups::DISPLAY)
            .section(SECTION)
            .description("sleep.setting.show.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                FOLDER,
                "sleep.setting.folder",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("sleep")),
            )
            .group(groups::DISPLAY)
            .section(SECTION)
            .editor(CHOOSE_FOLDER)
            .description("sleep.setting.folder.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                WIDGET_LIST,
                "sleep.setting.widgets",
                SettingKind::List,
                SettingValue::Text(Cow::Borrowed(
                    "home.date, home.continue, stats.widget.today, home.recent",
                )),
            )
            .group(groups::DISPLAY)
            .section(SECTION)
            .editor(EDIT_WIDGETS)
            .description("sleep.setting.widgets.help"),
        )?;
        for item in [SHOW, FOLDER, WIDGET_LIST] {
            r.set_menu_need(item, MenuNeed::Sleep)?;
        }
        // The book read last, for its cover.
        r.subscribe(
            events::BOOK_OPENED,
            Box::new(|env, params, _| {
                if let Some(path) = params.get(params::PATH) {
                    let _ = env.storage.write(&data("book.txt"), path.as_bytes());
                }
            }),
        )?;
        r.subscribe(
            events::BOOK_MOVED,
            Box::new(|env, params, _| {
                let (Some(from), Some(to)) = (params.get(params::FROM), params.get(params::PATH))
                else {
                    return;
                };
                if last_book(env.storage).as_deref() == Some(from) {
                    let _ = env.storage.write(&data("book.txt"), to.as_bytes());
                }
            }),
        )?;
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("sleep.title"),
            factory: Box::new(|env, _| Box::new(SleepScreen::new(env))),
        })?;
        r.bind_slot(slots::SLEEP, SCREEN, Priority::DEFAULT)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

fn last_book(storage: &dyn Storage) -> Option<String> {
    let data = quireink_sdk::read_file(storage, &data("book.txt"), 1024).ok()?;
    String::from_utf8(data).ok().filter(|p| !p.is_empty())
}

/// The cover of the book read last. Where it is is kept, so the book
/// is opened for it only once (memory is short while a book is open).
fn cover(env: &Env<'_>) -> Option<ImageRef> {
    let path = last_book(env.storage)?;
    let kept = quireink_sdk::read_file(env.storage, &data("cover.txt"), 2048)
        .ok()
        .and_then(|d| String::from_utf8(d).ok())
        .and_then(|text| {
            let mut f = text.splitn(4, '\t');
            let (book, width, height, image) = (f.next()?, f.next()?, f.next()?, f.next()?);
            if book != path || !env.storage.exists(image) {
                return None;
            }
            Some(ImageRef {
                path: image.to_string(),
                width: width.parse().ok()?,
                height: height.parse().ok()?,
            })
        });
    if kept.is_some() {
        return kept;
    }
    let document = env.formats.open(env.storage, &path).ok()?;
    let image = document.cover(env.storage).ok().flatten()?;
    let line = format!("{path}\t{}\t{}\t{}", image.width, image.height, image.path);
    let _ = env.storage.write(&data("cover.txt"), line.as_bytes());
    Some(image)
}

/// The next picture of the folder, decoded to fit the screen.
fn picture(env: &Env<'_>) -> Option<ImageRef> {
    let folder = env
        .settings
        .text(FOLDER.as_str())
        .trim_matches('/')
        .to_string();
    let mut pictures = Vec::new();
    let _ = env.storage.list(&folder, &mut |entry| {
        let name = entry.name.to_lowercase();
        if !entry.is_dir
            && !entry.name.starts_with('.')
            && [".jpg", ".jpeg", ".png", ".gif"]
                .iter()
                .any(|e| name.ends_with(e))
        {
            pictures.push(entry.name.to_string());
        }
    });
    if pictures.is_empty() {
        return None;
    }
    pictures.sort_unstable();
    // One after the other, from where the last sleep left off.
    let next = quireink_sdk::read_file(env.storage, &data("next.txt"), 32)
        .ok()
        .and_then(|n| String::from_utf8(n).ok()?.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let name = &pictures[next % pictures.len()];
    let _ = env
        .storage
        .write(&data("next.txt"), format!("{}", next + 1).as_bytes());
    let display = env.device.display;
    let side = u32::from(display.width.max(display.height));
    // In colour on colour screens (ADR 0015).
    quireink_sdk::image::picture_from_file(
        env.storage,
        &format!("{folder}/{name}"),
        &data("picture.gray"),
        side,
        env.device.display.format.is_color(),
    )
}

/// The widgets of [`WIDGET_LIST`] between the margins, together in the
/// middle of the screen, or where free spaces push them; `false` if none
/// shows.
fn draw_widgets(canvas: &mut Canvas<'_>, env: &Env<'_>) -> bool {
    let style = UiStyle::new(env);
    let display = env.device.display;
    let margin = style.margin as u16;
    let gap = style.gap as u16;
    let width = display.width.saturating_sub(2 * margin);
    // Free spaces (`None`) share the room the others leave.
    let mut placed: Vec<(&str, Option<u16>)> = Vec::new();
    let mut total = 0u16;
    let mut shown = 0;
    for id in quireink_sdk::status::item_ids(env.settings.text(WIDGET_LIST.as_str())) {
        if id == quireink_sdk::widgets::FILL {
            placed.push((id, None));
            continue;
        }
        let Some(height) = env.widgets.height(id, env, width).filter(|h| *h > 0) else {
            continue;
        };
        let added = height + if shown == 0 { 0 } else { gap * 2 };
        if total + added > display.height {
            break;
        }
        total += added;
        shown += 1;
        placed.push((id, Some(height)));
    }
    if shown == 0 {
        return false;
    }
    let fills = placed.iter().filter(|(_, h)| h.is_none()).count();
    let fill =
        quireink_sdk::widgets::fill_height(u32::from(display.height), u32::from(total), fills);
    let mut y = if fills == 0 {
        (display.height - total) / 2
    } else {
        0
    };
    let mut drawn = 0;
    for (id, height) in placed {
        let Some(height) = height else {
            y += fill;
            continue;
        };
        if drawn > 0 {
            y += gap * 2;
        }
        let area = Rect::new(margin, y, width, height);
        env.widgets.draw(id, canvas, env, area);
        y += height;
        drawn += 1;
    }
    true
}

/// What the screen shows while the device sleeps.
struct SleepScreen {
    show: String,
    image: Option<ImageRef>,
}

impl SleepScreen {
    fn new(env: &Env<'_>) -> Self {
        let show = env.settings.text(SHOW.as_str()).to_string();
        let image = match show.as_str() {
            PICTURE => picture(env),
            MESSAGE | PAGE | WIDGETS => None,
            _ => cover(env),
        };
        Self { show, image }
    }
}

impl Screen for SleepScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        // The page stays: nothing is drawn over it.
        (self.show == PAGE).then_some(Rect::new(0, 0, 0, 0))
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        if self.show == PAGE {
            return;
        }
        let display = env.device.display;
        let screen = Rect::new(0, 0, display.width, display.height);
        if self.show == WIDGETS && draw_widgets(canvas, env) {
            return;
        }
        match &self.image {
            Some(image) => {
                // As large as fits, in the middle, on black.
                ui::fill(canvas, screen, Color::BLACK);
                let (w, h) = ui::fit_image(image, display.width, display.height);
                let area = Rect::new((display.width - w) / 2, (display.height - h) / 2, w, h);
                ui::draw_image(canvas, env.storage, image, area);
            }
            None => {
                let style = UiStyle::new(env);
                ui::draw_message(canvas, env, &style, screen, env.i18n.tr("sleep.sleeping"));
            }
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if self.show != PAGE {
            cx.request_refresh(RefreshMode::Full);
        }
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        // Nothing to do while asleep.
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        if *event == events::RESUME {
            cx.command(Command::Back);
            if self.show != PAGE {
                cx.request_refresh(RefreshMode::Full);
            }
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
