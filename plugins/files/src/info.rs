//! Book information: cover, title and author, the series and tags, the
//! language, how far it is read, the file, and what the book is about.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::document::ImageRef;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, Screen,
    files,
};

use quireink_sdk::library;

/// What is shown of a book.
#[derive(Debug, Default)]
struct Details {
    title: String,
    author: String,
    cover: Option<ImageRef>,
    /// Label key and value, in order.
    rows: Vec<(&'static str, String)>,
    description: String,
}

pub(crate) struct InfoScreen {
    style: UiStyle,
    header: ui::Header,
    details: Details,
}

impl InfoScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let path = params.get(params::PATH).unwrap_or_default();
        Self {
            header: ui::Header::new(env, &style, true),
            details: details(env, path),
            style,
        }
    }
}

/// Reads what is known of the book at `path`.
fn details(env: &Env<'_>, path: &str) -> Details {
    let storage = env.storage;
    let name = files::name(path);
    let cached = library::book(storage, path);
    let mut details = Details {
        title: name.rsplit_once('.').map_or(name, |(n, _)| n).to_string(),
        ..Details::default()
    };
    let mut rows = Vec::new();
    if let Ok(document) = env.formats.open(storage, path) {
        let metadata = document.metadata().clone();
        if let Some(title) = metadata.title {
            details.title = title;
        }
        details.author = metadata.authors.join(", ");
        details.cover = document.cover(storage).ok().flatten();
        if let Some(series) = metadata.series {
            let value = match metadata.series_index {
                Some(number) => format!(
                    "{series} · {}",
                    env.i18n
                        .format("files.series-number", &[("number", Arg::Str(&number))])
                ),
                None => series,
            };
            rows.push(("files.info.series", value));
        }
        if !metadata.subjects.is_empty() {
            rows.push(("files.info.tags", metadata.subjects.join(", ")));
        }
        if let Some(language) = metadata.language {
            rows.push(("files.info.language", language));
        }
        details.description = metadata.description.unwrap_or_default();
    }
    if let Some(progress) = cached.and_then(|b| b.progress) {
        rows.push(("files.info.progress", format!("{progress} %")));
    }
    let format = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_uppercase())
        .unwrap_or_default();
    let size = storage.size(path).unwrap_or_default();
    rows.push(("files.info.file", format!("{format} · {}", size_text(size))));
    rows.push(("files.info.folder", files::parent(path).to_string()));
    details.rows = rows;
    details
}

/// "340 KB", "2.4 MB".
fn size_text(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        let tenths = (bytes * 10 + 512 * 1024) / (1024 * 1024);
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

impl Screen for InfoScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header
            .draw(canvas, env, &style, env.i18n.tr("files.info"), None, None);
        let Some(font) = style.font(env) else {
            return;
        };
        let display = env.device.display;
        let margin = style.margin as u16;
        let gap = style.gap as u16;
        let width = display.width.saturating_sub(2 * margin);
        let mut y = self.header.area.height + gap * 2;
        let x = f32::from(margin);
        let line = |size: f32| font.line_metrics(size).line_height as u16;
        let d = &self.details;

        // The cover beside title and author.
        let cover_height = style.row as u16 * 2;
        let mut text_x = margin;
        if let Some(cover) = &d.cover {
            let (w, h) = ui::fit_image(cover, cover_height * 3 / 4, cover_height);
            ui::draw_image(canvas, env.storage, cover, Rect::new(margin, y, w, h));
            text_x = margin + w + gap * 2;
        }
        let text_width = display.width.saturating_sub(text_x + margin);
        let mut ty = y;
        for range in font
            .wrap(style.title, &d.title, f32::from(text_width))
            .into_iter()
            .take(2)
        {
            let row = Rect::new(text_x, ty, text_width, line(style.title));
            let text = &d.title[range];
            ui::draw_label(
                canvas,
                &font,
                style.title,
                f32::from(text_x),
                row,
                text,
                Color::BLACK,
            );
            ty += line(style.title);
        }
        if !d.author.is_empty() {
            let row = Rect::new(text_x, ty, text_width, line(style.text));
            ui::draw_label(
                canvas,
                &font,
                style.text,
                f32::from(text_x),
                row,
                &d.author,
                style.faint(80),
            );
            ty += line(style.text);
        }
        y = ty.max(if d.cover.is_some() {
            y + cover_height
        } else {
            y
        }) + gap * 2;

        // Label and value in two columns, the values aligned.
        let label_width = d
            .rows
            .iter()
            .map(|(key, _)| font.measure(style.small, env.i18n.tr(key)))
            .fold(0.0, f32::max)
            + f32::from(gap) * 2.0;
        // Label and value share a baseline.
        let ascent = font.line_metrics(style.text).ascent;
        for (key, value) in &d.rows {
            let baseline = i32::from(y) + ascent as i32;
            let label = env.i18n.tr(key);
            font.draw(canvas, style.small, x, baseline, label, style.faint(80));
            let room = f32::from(width) - label_width;
            let value = ui::fit(&font, style.text, value, room);
            font.draw(
                canvas,
                style.text,
                x + label_width,
                baseline,
                &value,
                Color::BLACK,
            );
            y += line(style.text) + gap / 2;
        }

        // What the book is about, as far as there is room.
        if d.description.is_empty() {
            return;
        }
        y += gap * 2;
        let bottom = display.height.saturating_sub(margin);
        let lines = font.wrap(style.text, &d.description, f32::from(width));
        let fits = usize::from(bottom.saturating_sub(y) / line(style.text).max(1));
        for (i, range) in lines.iter().take(fits).enumerate() {
            let mut text = d.description[range.clone()].to_string();
            if i + 1 == fits && lines.len() > fits {
                text.push('…');
            }
            let row = Rect::new(margin, y, width, line(style.text));
            ui::draw_label(canvas, &font, style.text, x, row, &text, Color::BLACK);
            y += line(style.text);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if [actions::BACK, actions::CONFIRM, actions::MENU].contains(action) {
            cx.command(Command::Back);
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(action) = self.header.tap(cx.env(), &self.style, gesture.start, None) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        Handled::No
    }
}

#[cfg(test)]
mod tests {
    use super::size_text;

    #[test]
    fn sizes_read_easily() {
        assert_eq!(size_text(1000), "1 KB");
        assert_eq!(size_text(340 * 1024), "340 KB");
        assert_eq!(size_text(2_516_582), "2.4 MB");
    }
}
