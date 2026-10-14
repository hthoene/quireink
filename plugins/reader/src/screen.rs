use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, fonts, params};
use quireink_sdk::document::{Document, DocumentError, Locator};
use quireink_sdk::layout::{Page, Style, layout_page};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, FontHandle, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

use crate::events_out::{BOOK_OPENED, PAGE_TURNED};
use crate::index::PageIndex;
use crate::settings::{FONT_SIZE, INDENT, JUSTIFY, LINE_SPACING, MARGIN};
use crate::{PLUGIN, state_path};

/// Pages laid out per background step; small enough to keep input snappy.
const PAGES_PER_STEP: usize = 8;
/// Pause between background steps.
const STEP_DELAY_MS: u64 = 2;
/// Pages remembered for going back before the index covers them.
const HISTORY: usize = 64;

/// Page geometry and text style, derived from settings and the screen.
#[derive(Debug, Clone)]
struct Typography {
    size: f32,
    style: Style,
    /// Content box.
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    /// Status line below the content.
    status: Rect,
    /// Identifies this layout; part of the page index file name.
    key: String,
}

impl Typography {
    fn new(env: &Env<'_>, font: &FontHandle<'_>) -> Self {
        let settings = env.settings;
        let m = env.metrics;
        let size = m.pt(settings.int(FONT_SIZE.as_str()) as f32);
        let metrics = font.line_metrics(size);
        let natural = metrics.ascent + metrics.descent;
        let line_height = (size * settings.int(LINE_SPACING.as_str()) as f32 / 100.0).max(natural);
        let indent = settings.bool(INDENT.as_str());
        let justify = settings.bool(JUSTIFY.as_str());
        let style = Style {
            ascent: metrics.ascent + (line_height - natural) / 2.0,
            line_height,
            paragraph_spacing: if indent { 0.0 } else { line_height * 0.6 },
            indent: if indent { size * 1.5 } else { 0.0 },
            justify,
        };

        let display = env.device.display;
        let margin = m.mm(settings.int(MARGIN.as_str()).max(0) as u32) as f32;
        let status_height = m.mm(6) as f32;
        let width = (f32::from(display.width) - 2.0 * margin).max(1.0);
        let height = (f32::from(display.height) - 2.0 * margin - status_height).max(1.0);
        let status = Rect::new(
            margin as u16,
            (margin + height) as u16,
            width as u16,
            status_height as u16,
        );
        let key = format!(
            "{:016x}",
            fnv(&format!(
                "{}|{size}|{line_height}|{}|{}|{justify}|{width}x{height}",
                font.id(),
                style.indent,
                style.paragraph_spacing,
            ))
        );
        Self {
            size,
            style,
            x: margin,
            y: margin,
            width,
            height,
            status,
            key,
        }
    }
}

/// The book being read.
struct Book {
    path: String,
    /// Directory for this book's data.
    dir: String,
    title: String,
    document: Box<dyn Document>,
    typography: Typography,
    page: Page,
    history: Vec<Locator>,
    index: Option<PageIndex>,
    /// Where background pagination continues.
    indexing: Option<Locator>,
}

/// Shows a book page by page.
pub(crate) struct ReaderScreen {
    book: Result<Book, String>,
}

impl ReaderScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        Self {
            book: Book::open(env, params),
        }
    }
}

impl Book {
    /// Opens the book named by the `path` parameter; errors are messages
    /// to show instead of the book.
    fn open(env: &Env<'_>, params: &Params) -> Result<Book, String> {
        let path = params
            .get(params::PATH)
            .ok_or_else(|| env.i18n.tr("reader.no-book").to_string())?;
        let error = |e: DocumentError| format!("{}\n{e}", env.i18n.tr("reader.error"));
        let document = env.formats.open(env.storage, path).map_err(error)?;
        let font = env
            .fonts
            .role(fonts::READING.as_str())
            .ok_or_else(|| env.i18n.tr("reader.no-font").to_string())?;
        let typography = Typography::new(env, &font);
        let dir = format!(
            "{}/books/{:016x}",
            Env::plugin_dir(PLUGIN.as_str()),
            fnv(path)
        );
        let title = document
            .metadata()
            .title
            .clone()
            .unwrap_or_else(|| path.to_string());

        let mut book = Book {
            path: path.to_string(),
            index: None,
            indexing: None,
            page: Page::default(),
            history: Vec::new(),
            dir,
            title,
            document,
            typography,
        };
        let start = book.saved_position(env).unwrap_or(Locator::START);
        book.page = match book.layout(env, start).map_err(error)? {
            page if page.is_past_end() && start != Locator::START => {
                book.layout(env, Locator::START).map_err(error)?
            }
            page => page,
        };
        book.index = Some(PageIndex::open(
            env.storage,
            format!("{}/pages-{}.bin", book.dir, book.typography.key),
        ));
        let state = format!("[reader]\nlast = \"{}\"\n", escape(&book.path));
        let _ = env.storage.write(&state_path(), state.as_bytes());
        Ok(book)
    }

    fn font<'a>(&self, env: &Env<'a>) -> Option<FontHandle<'a>> {
        env.fonts.role(fonts::READING.as_str())
    }

    fn layout(&self, env: &Env<'_>, at: Locator) -> Result<Page, DocumentError> {
        let Some(font) = self.font(env) else {
            return Ok(Page::default());
        };
        let t = &self.typography;
        let advance = |c: char| font.advance(t.size, c);
        layout_page(
            self.document.as_ref(),
            env.storage,
            at,
            t.width,
            t.height,
            &t.style,
            &advance,
        )
    }

    fn progress_path(&self) -> String {
        format!("{}/progress.ini", self.dir)
    }

    fn saved_position(&self, env: &Env<'_>) -> Option<Locator> {
        let data = quireink_sdk::read_file(env.storage, &self.progress_path(), 4096).ok()?;
        let text = core::str::from_utf8(&data).ok()?;
        let (mut section, mut offset) = (None, None);
        for entry in quireink_sdk::config::parse(text).filter_map(Result::ok) {
            match (entry.section, entry.key) {
                ("progress", "section") => section = entry.value.as_u32(),
                ("progress", "offset") => offset = entry.value.as_u32(),
                _ => {}
            }
        }
        Some(Locator::new(section?, offset?))
    }

    fn percent(&self) -> f32 {
        self.document.progress(self.page.start) * 100.0
    }

    fn save_position(&self, env: &Env<'_>) {
        let start = self.page.start;
        let text = format!(
            "[progress]\nsection = {}\noffset = {}\npercent = {:.1}\n",
            start.section,
            start.offset,
            self.percent()
        );
        let _ = env.storage.write(&self.progress_path(), text.as_bytes());
    }

    /// The start of the previous page: the page shown before (this
    /// session), else from the page index.
    fn previous_start(&mut self, env: &Env<'_>) -> Option<Locator> {
        if let Some(start) = self.history.pop() {
            return Some(start);
        }
        let index = self.index.as_ref()?;
        match index.locate(env.storage, self.page.start)? {
            (page, true) if page > 0 => index.entry(env.storage, page - 1),
            (page, false) => index.entry(env.storage, page),
            _ => None,
        }
    }

    fn show(&mut self, cx: &mut Cx<'_, '_>, start: Locator, forward: bool) -> bool {
        let env = *cx.env();
        let Ok(page) = self.layout(&env, start) else {
            return false;
        };
        if page.is_past_end() || page.start == self.page.start {
            return false;
        }
        if forward {
            if self.history.len() == HISTORY {
                self.history.remove(0);
            }
            self.history.push(self.page.start);
        }
        self.page = page;
        self.save_position(&env);
        let progress = format!("{:.1}", self.percent());
        cx.command(Command::Emit(
            PAGE_TURNED,
            Params::new()
                .with(params::PATH, self.path.clone())
                .with("progress", progress),
        ));
        cx.request_redraw();
        true
    }

    /// One step of background pagination. Returns whether more work remains.
    fn index_step(&mut self, env: &Env<'_>) -> bool {
        let Some(mut index) = self.index.take() else {
            return false;
        };
        if index.is_complete() {
            self.index = Some(index);
            return false;
        }
        let mut next = match self.indexing {
            Some(next) => next,
            None => match index.last(env.storage) {
                Some(last) => match self.layout(env, last) {
                    Ok(page) => page.end,
                    Err(_) => return false,
                },
                None => Locator::START,
            },
        };
        let mut starts = Vec::with_capacity(PAGES_PER_STEP);
        let mut done = false;
        for _ in 0..PAGES_PER_STEP {
            let Ok(page) = self.layout(env, next) else {
                // Unreadable: give up indexing, reading still works.
                return false;
            };
            if page.is_past_end() {
                done = true;
                break;
            }
            starts.push(page.start);
            if page.end <= page.start {
                done = true;
                break;
            }
            next = page.end;
        }
        if !index.append(env.storage, &starts) {
            // Read-only storage: no index.
            return false;
        }
        if done {
            index.finish(env.storage);
        }
        self.indexing = Some(next);
        self.index = Some(index);
        !done
    }

    /// "12 / 340" once the index covers the page exactly, else a percentage.
    fn position_label(&self, env: &Env<'_>) -> String {
        if let Some(index) = &self.index
            && index.is_complete()
            && let Some((page, true)) = index.locate(env.storage, self.page.start)
        {
            return env.i18n.format(
                "reader.page",
                &[
                    ("page", Arg::Int(i64::from(page) + 1)),
                    ("pages", Arg::Int(i64::from(index.pages()))),
                ],
            );
        }
        env.i18n.format(
            "reader.percent",
            &[("percent", Arg::Int(self.percent() as i64))],
        )
    }

    fn draw_page(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let Some(font) = self.font(env) else {
            return;
        };
        let t = &self.typography;
        let space = font.advance(t.size, ' ');
        for line in &self.page.lines {
            let baseline = (t.y + line.baseline + 0.5) as i32;
            let mut x = t.x + line.x;
            if line.word_spacing == 0.0 {
                font.draw(canvas, t.size, x, baseline, &line.text, Color::BLACK);
                continue;
            }
            for (i, word) in line.text.split(' ').enumerate() {
                if i > 0 {
                    x += space + line.word_spacing;
                }
                x += font.draw(canvas, t.size, x, baseline, word, Color::BLACK);
            }
        }
    }

    fn draw_status(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let r = self.typography.status;
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
            Color::WHITE,
        );
        let Some(font) = env.fonts.role(fonts::UI.as_str()) else {
            return;
        };
        let size = env.metrics.pt(8.0);
        let metrics = font.line_metrics(size);
        let baseline = i32::from(r.y)
            + ((f32::from(r.height) + metrics.ascent - metrics.descent) / 2.0) as i32;
        let gray = Color::gray(80);
        let label = self.position_label(env);
        let label_width = font.measure(size, &label);
        let right = f32::from(r.x) + f32::from(r.width) - label_width;
        font.draw(canvas, size, right, baseline, &label, gray);

        let available = (right - f32::from(r.x) - env.metrics.mm(4) as f32).max(0.0);
        let title = fit(&font, size, &self.title, available);
        font.draw(canvas, size, f32::from(r.x), baseline, &title, gray);
    }
}

impl Screen for ReaderScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        match &self.book {
            Ok(book) => {
                book.draw_page(canvas, env);
                book.draw_status(canvas, env);
            }
            Err(message) => draw_message(canvas, env, message),
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if let Ok(book) = &self.book {
            let path = book.path.clone();
            cx.command(Command::Emit(
                BOOK_OPENED,
                Params::new().with(params::PATH, path),
            ));
            let now = cx.env().now;
            cx.wake_at(now);
        }
    }

    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &mut self.book else {
            return;
        };
        if book.index_step(&env) {
            cx.wake_at(env.now.add_millis(STEP_DELAY_MS));
        } else if book.index.as_ref().is_some_and(PageIndex::is_complete) {
            // Page numbers are known now: update just the status line.
            book.draw_status(cx.canvas(), &env);
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let Ok(book) = &mut self.book else {
            return Handled::No;
        };
        if *action == actions::NEXT_PAGE {
            let end = book.page.end;
            book.show(cx, end, true);
            Handled::Yes
        } else if *action == actions::PREV_PAGE {
            let env = *cx.env();
            if let Some(start) = book.previous_start(&env) {
                book.show(cx, start, false);
            }
            Handled::Yes
        } else {
            Handled::No
        }
    }
}

/// Centred, wrapped message text (errors, missing book).
fn draw_message(canvas: &mut Canvas<'_>, env: &Env<'_>, message: &str) {
    let Some(font) = env.fonts.role(fonts::UI.as_str()) else {
        return;
    };
    let display = env.device.display;
    let size = env.metrics.pt(12.0);
    let margin = env.metrics.mm(8) as f32;
    let width = f32::from(display.width) - 2.0 * margin;
    let line_height = font.line_metrics(size).line_height;
    let lines = font.wrap(size, message, width);
    let mut baseline =
        (f32::from(display.height) - line_height * lines.len() as f32) / 2.0 + line_height;
    for range in lines {
        let text = &message[range];
        let x = (f32::from(display.width) - font.measure(size, text)) / 2.0;
        font.draw(canvas, size, x, baseline as i32, text, Color::BLACK);
        baseline += line_height;
    }
}

/// `text` shortened with an ellipsis to fit `width`.
fn fit(font: &FontHandle<'_>, size: f32, text: &str, width: f32) -> String {
    if font.measure(size, text) <= width {
        return text.to_string();
    }
    let mut result = String::new();
    let ellipsis = font.measure(size, "…");
    let mut used = 0.0;
    for c in text.chars() {
        let w = font.advance(size, c);
        if used + w + ellipsis > width {
            break;
        }
        used += w;
        result.push(c);
    }
    result.push('…');
    result
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// FNV-1a, for short stable ids of paths and layouts.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}
