use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, fonts, params, slots};
use quireink_sdk::document::{Document, DocumentError, Locator};
use quireink_sdk::layout::{Face, Hyphenate, Page, PageImage, Style, Typesetter, layout_page};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, FontHandle, Gesture, GestureKind, Handled, Id, Params,
    Point, Rect, RefreshMode, Screen,
};

use crate::index::PageIndex;
use crate::settings::{FONT_SIZE, HYPHENATE, INDENT, JUSTIFY, LINE_SPACING, MARGIN};
use crate::{PLUGIN, state_path};

/// Pages laid out per background step; small enough to keep input snappy.
const PAGES_PER_STEP: usize = 8;
/// Pause between background steps.
const STEP_DELAY_MS: u64 = 2;
/// Pages remembered for going back before the index covers them.
const HISTORY: usize = 64;

/// Page geometry and text style, derived from settings and the screen.
/// Relative sizes of headings 1 to 6.
const HEADING_SCALE: [f32; 6] = [1.6, 1.4, 1.2, 1.1, 1.0, 1.0];

/// The reading fonts by face, for layout and drawing.
struct Faces<'a> {
    /// Regular, bold, italic and bold italic.
    fonts: [FontHandle<'a>; 4],
    size: f32,
    ascent: f32,
    line_height: f32,
}

impl<'a> Faces<'a> {
    /// The reading faces; missing ones fall back to the nearest face, and
    /// a font the user chose for book text is not mixed with the default
    /// bold or italic faces.
    fn new(env: &Env<'a>, t: &Typography) -> Option<Self> {
        let fonts_ = env.fonts;
        let regular = fonts_.role(fonts::READING.as_str())?;
        let custom = fonts_.user_bound(fonts::READING.as_str());
        let face = |role: &quireink_sdk::Id| {
            if custom && !fonts_.user_bound(role.as_str()) {
                None
            } else {
                fonts_.bound(role.as_str())
            }
        };
        let bold = face(&fonts::READING_BOLD).unwrap_or(regular);
        let italic = face(&fonts::READING_ITALIC).unwrap_or(regular);
        let bold_italic = face(&fonts::READING_BOLD_ITALIC).unwrap_or(bold);
        Some(Self {
            fonts: [regular, bold, italic, bold_italic],
            size: t.size,
            ascent: t.ascent,
            line_height: t.line_height,
        })
    }

    fn font(&self, face: Face) -> &FontHandle<'a> {
        &self.fonts[usize::from(face.style.bits() & 3)]
    }

    fn size(&self, face: Face) -> f32 {
        match face.heading {
            0 => self.size,
            level => self.size * HEADING_SCALE[usize::from(level.clamp(1, 6)) - 1],
        }
    }
}

impl Typesetter for Faces<'_> {
    fn advance(&self, face: Face, c: char) -> f32 {
        self.font(face).advance(self.size(face), c)
    }

    fn line(&self, face: Face) -> (f32, f32) {
        if face.heading == 0 {
            return (self.ascent, self.line_height);
        }
        let size = self.size(face);
        let metrics = self.font(face).line_metrics(size);
        let natural = metrics.ascent + metrics.descent;
        let line_height = (size * 1.2).max(natural);
        (metrics.ascent + (line_height - natural) / 2.0, line_height)
    }
}

#[derive(Debug, Clone)]
struct Typography {
    size: f32,
    /// Body text lines: baseline from the top of a line, line height.
    ascent: f32,
    line_height: f32,
    style: Style,
    /// Content box.
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    /// Status line below the content.
    status: Rect,
    /// Language whose hyphenation patterns are used, if hyphenating.
    hyphenation: Option<String>,
    /// Identifies this layout; part of the page index file name.
    key: String,
}

impl Typography {
    /// Typography for a book in `language`.
    fn new(env: &Env<'_>, font: &FontHandle<'_>, language: &str) -> Self {
        let settings = env.settings;
        let m = env.metrics;
        let size = m.pt(settings.int(FONT_SIZE.as_str()) as f32);
        let metrics = font.line_metrics(size);
        let natural = metrics.ascent + metrics.descent;
        let line_height = (size * settings.int(LINE_SPACING.as_str()) as f32 / 100.0).max(natural);
        let indent = settings.bool(INDENT.as_str());
        let justify = settings.bool(JUSTIFY.as_str());
        let ascent = metrics.ascent + (line_height - natural) / 2.0;
        let style = Style {
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
        let hyphenation = (settings.bool(HYPHENATE.as_str())
            && env.hyphenation.get(language).is_some())
        .then(|| language.to_string());
        let key = format!(
            "{:016x}",
            fnv(&format!(
                "{}|{}|{}|{}|{size}|{line_height}|{}|{}|{justify}|{width}x{height}|{}",
                font.id(),
                face_id(env, &fonts::READING_BOLD),
                face_id(env, &fonts::READING_ITALIC),
                face_id(env, &fonts::READING_BOLD_ITALIC),
                style.indent,
                style.paragraph_spacing,
                hyphenation.as_deref().unwrap_or("-"),
            ))
        );
        Self {
            size,
            ascent,
            line_height,
            style,
            x: margin,
            y: margin,
            width,
            height,
            status,
            hyphenation,
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
    /// The book's language, or the UI language if it declares none.
    language: String,
    typography: Typography,
    page: Page,
    history: Vec<Locator>,
    index: Option<PageIndex>,
    /// Where background pagination continues.
    indexing: Option<Locator>,
}

/// A word on the page, with its box on the screen.
#[derive(Debug, Clone, PartialEq)]
struct WordBox {
    area: Rect,
    text: String,
}

impl WordBox {
    /// The word without surrounding punctuation, as looked up.
    fn lookup_text(&self) -> &str {
        self.text.trim_matches(|c: char| !c.is_alphanumeric())
    }
}

/// Shows a book page by page.
pub(crate) struct ReaderScreen {
    book: Result<Book, String>,
    /// The word chosen with buttons to look up, as an index into the
    /// page's words.
    selected: Option<usize>,
}

impl ReaderScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        Self {
            book: Book::open(env, params),
            selected: None,
        }
    }

    /// Opens the lookup slot (e.g. the dictionary) for `word`.
    fn look_up(book: &Book, word: &WordBox, cx: &mut Cx<'_, '_>) {
        let text = word.lookup_text();
        if text.is_empty() {
            return;
        }
        cx.command(Command::OpenSlot(
            slots::LOOKUP,
            Params::new()
                .with(params::WORD, text.to_string())
                .with(params::LANGUAGE, book.language.clone()),
        ));
    }

    /// Moves the selection to word `to`, redrawing both highlights.
    fn select(&mut self, words: &[WordBox], to: Option<usize>, cx: &mut Cx<'_, '_>) {
        for index in [self.selected, to].into_iter().flatten() {
            if let Some(word) = words.get(index) {
                cx.request_redraw_area(highlight(word.area));
            }
        }
        self.selected = to;
        cx.request_refresh(RefreshMode::Fast);
    }

    /// Buttons while a word is selected: move, look up, or stop.
    fn on_selection(&mut self, action: &Id, index: usize, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return Handled::No;
        };
        let words = book.words(&env);
        if words.is_empty() {
            self.selected = None;
            return Handled::No;
        }
        let last = words.len() - 1;
        let a = action;
        let to = if [actions::LEFT, actions::PREVIOUS, actions::PREV_PAGE].contains(a) {
            index.saturating_sub(1)
        } else if [actions::RIGHT, actions::NEXT, actions::NEXT_PAGE].contains(a) {
            (index + 1).min(last)
        } else if *a == actions::UP {
            nearest_on_line(&words, index, false)
        } else if *a == actions::DOWN {
            nearest_on_line(&words, index, true)
        } else if *a == actions::CONFIRM {
            if let Some(word) = words.get(index) {
                Self::look_up(book, word, cx);
            }
            return Handled::Yes;
        } else if *a == actions::BACK || *a == actions::MENU {
            self.select(&words, None, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if to != index {
            self.select(&words, Some(to), cx);
        }
        Handled::Yes
    }
}

/// The area redrawn around a highlighted word.
fn highlight(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_sub(6),
        area.y.saturating_sub(6),
        area.width + 12,
        area.height + 12,
    )
}

/// The word on the line above (or below) word `index` nearest to it.
fn nearest_on_line(words: &[WordBox], index: usize, below: bool) -> usize {
    let current = &words[index].area;
    let center = |r: &Rect| i32::from(r.x) + i32::from(r.width) / 2;
    let line = words
        .iter()
        .map(|w| w.area.y)
        .filter(|&y| if below { y > current.y } else { y < current.y })
        .min_by_key(|&y| y.abs_diff(current.y));
    let Some(line) = line else {
        return index;
    };
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| w.area.y == line)
        .min_by_key(|(_, w)| center(&w.area).abs_diff(center(current)))
        .map_or(index, |(i, _)| i)
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
        let language = document
            .metadata()
            .language
            .clone()
            .unwrap_or_else(|| env.i18n.language().to_string());
        let typography = Typography::new(env, &font, &language);
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
            language,
            typography,
        };
        let start = book.saved_position(env).unwrap_or(Locator::START);
        book.page = match book.layout(env, start).map_err(error)? {
            page if page.is_past_end() && start != Locator::START => {
                book.layout(env, Locator::START).map_err(error)?
            }
            page => page,
        };
        book.index = Some(book.open_index(env));
        let state = format!("[reader]\nlast = \"{}\"\n", escape(&book.path));
        let _ = env.storage.write(&state_path(), state.as_bytes());
        Ok(book)
    }

    /// Parameters of the standard reading events.
    fn event_params(&self) -> Params {
        Params::new()
            .with(params::PATH, self.path.clone())
            .with(params::TITLE, self.title.clone())
            .with(params::PROGRESS, format!("{:.1}", self.percent()))
    }

    fn open_index(&self, env: &Env<'_>) -> PageIndex {
        PageIndex::open(
            env.storage,
            format!("{}/pages-{}.bin", self.dir, self.typography.key),
        )
    }

    /// Applies changed reading settings: new typography, the current
    /// position laid out again, and a page index for the new layout.
    fn relayout(&mut self, env: &Env<'_>) {
        let Some(font) = self.font(env) else {
            return;
        };
        self.typography = Typography::new(env, &font, &self.language);
        if let Ok(page) = self.layout(env, self.page.start) {
            self.page = page;
        }
        self.history.clear();
        self.index = Some(self.open_index(env));
        self.indexing = None;
    }

    fn font<'a>(&self, env: &Env<'a>) -> Option<FontHandle<'a>> {
        env.fonts.role(fonts::READING.as_str())
    }

    fn layout(&self, env: &Env<'_>, at: Locator) -> Result<Page, DocumentError> {
        let t = &self.typography;
        let Some(faces) = Faces::new(env, t) else {
            return Ok(Page::default());
        };
        let patterns = t
            .hyphenation
            .as_deref()
            .and_then(|language| env.hyphenation.get(language));
        let hyphenate = |word: &str, out: &mut dyn FnMut(usize)| {
            if let Some(patterns) = patterns {
                patterns.hyphenate(word, out);
            }
        };
        layout_page(
            self.document.as_ref(),
            env.storage,
            at,
            t.width,
            t.height,
            &t.style,
            &faces,
            patterns.is_some().then_some(&hyphenate as Hyphenate<'_>),
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
        cx.command(Command::Emit(events::PAGE_TURNED, self.event_params()));
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
        let t = &self.typography;
        let Some(faces) = Faces::new(env, t) else {
            return;
        };
        let clip = canvas.clip();
        for image in &self.page.images {
            draw_image(canvas, env, t, image);
        }
        for line in &self.page.lines {
            let baseline = (t.y + line.baseline + 0.5) as i32;
            // Lines outside the part being drawn (e.g. another band).
            let top = baseline - (2.0 * t.line_height) as i32;
            let bottom = baseline + t.line_height as i32;
            if bottom < i32::from(clip.y) || top > clip.bottom() as i32 {
                continue;
            }
            let mut x = t.x + line.x;
            for run in &line.runs {
                let font = faces.font(run.face);
                let size = faces.size(run.face);
                let text = &line.text[run.start..run.end];
                if line.word_spacing == 0.0 {
                    x += font.draw(canvas, size, x, baseline, text, Color::BLACK);
                    continue;
                }
                let space = font.advance(size, ' ');
                for (i, word) in text.split(' ').enumerate() {
                    if i > 0 {
                        x += space + line.word_spacing;
                    }
                    x += font.draw(canvas, size, x, baseline, word, Color::BLACK);
                }
            }
        }
    }

    /// The words of the page with their boxes on the screen, in reading
    /// order, measured as they are drawn.
    fn words(&self, env: &Env<'_>) -> Vec<WordBox> {
        let t = &self.typography;
        let Some(faces) = Faces::new(env, t) else {
            return Vec::new();
        };
        let mut words: Vec<WordBox> = Vec::new();
        for line in &self.page.lines {
            let mut x = t.x + line.x;
            // Inside a word that continues in the next run (e.g. bold).
            let mut joined = false;
            for run in &line.runs {
                let font = faces.font(run.face);
                let size = faces.size(run.face);
                let (ascent, height) = faces.line(run.face);
                let top = t.y + line.baseline - ascent;
                let space = font.advance(size, ' ') + line.word_spacing;
                for (i, part) in line.text[run.start..run.end].split(' ').enumerate() {
                    if i > 0 {
                        x += space;
                        joined = false;
                    }
                    if part.is_empty() {
                        continue;
                    }
                    let width = font.measure(size, part);
                    match words.last_mut() {
                        Some(last) if joined => {
                            last.text.push_str(part);
                            last.area.width = (x + width) as u16 - last.area.x;
                        }
                        _ => words.push(WordBox {
                            area: Rect::new(x as u16, top as u16, width as u16, height as u16),
                            text: part.to_string(),
                        }),
                    }
                    x += width;
                    joined = true;
                }
            }
        }
        words
    }

    /// The word at `point`, if any.
    fn word_at(&self, env: &Env<'_>, point: Point) -> Option<WordBox> {
        self.words(env).into_iter().find(|word| {
            let area = word.area;
            // A little slack between words and lines.
            let slack = 4;
            point.x >= i32::from(area.x) - slack
                && point.x < area.right() as i32 + slack
                && point.y >= i32::from(area.y)
                && point.y < area.bottom() as i32
        })
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
                // The word chosen for looking up.
                if let Some(word) = self
                    .selected
                    .and_then(|i| book.words(env).into_iter().nth(i))
                {
                    let r = highlight(word.area);
                    let line = env.metrics.tenth_mm(4).max(2);
                    canvas.stroke_rect(
                        i32::from(r.x) + 2,
                        i32::from(r.y) + 2,
                        u32::from(r.width) - 4,
                        u32::from(r.height) - 4,
                        line,
                        Color::BLACK,
                    );
                }
            }
            Err(message) => draw_message(canvas, env, message),
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if let Ok(book) = &self.book {
            cx.command(Command::Emit(events::BOOK_OPENED, book.event_params()));
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
            cx.request_redraw_area(book.typography.status);
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if let Some(index) = self.selected {
            return self.on_selection(action, index, cx);
        }
        if *action == crate::actions::LOOKUP {
            // Choose a word with the buttons, starting at the first one.
            let env = *cx.env();
            if let Ok(book) = &self.book {
                let words = book.words(&env);
                let first = words.iter().position(|w| !w.lookup_text().is_empty());
                self.select(&words, first, cx);
            }
            return Handled::Yes;
        }
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
        } else if *action == crate::actions::TOC {
            cx.command(Command::OpenScreen(
                crate::TOC_SCREEN,
                crate::toc::locator_params(&book.path, book.page.start),
            ));
            Handled::Yes
        } else if *action == actions::MENU || *action == actions::CONFIRM {
            cx.command(Command::OpenSlot(
                slots::MENU,
                Params::new().with(params::MENU, crate::MENU.as_str()),
            ));
            Handled::Yes
        } else {
            Handled::No
        }
    }

    /// A long press looks up the word under the finger.
    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let Ok(book) = &self.book else {
            return Handled::No;
        };
        let env = *cx.env();
        match gesture.kind {
            GestureKind::LongPress => match book.word_at(&env, gesture.start) {
                Some(word) => {
                    Self::look_up(book, &word, cx);
                    Handled::Yes
                }
                None => Handled::No,
            },
            // While choosing a word with buttons, a tap looks up the word
            // tapped, or stops choosing.
            GestureKind::Tap if self.selected.is_some() => {
                match book.word_at(&env, gesture.start) {
                    Some(word) => Self::look_up(book, &word, cx),
                    None => {
                        let words = book.words(&env);
                        self.select(&words, None, cx);
                    }
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let Ok(book) = &mut self.book else {
            return;
        };
        if *event == crate::GOTO
            && params.get(params::PATH) == Some(book.path.as_str())
            && let Some(at) = crate::toc::locator_of(params)
        {
            // Another page: no word stays chosen.
            self.selected = None;
            book.show(cx, at, true);
            return;
        }
        let id = params.get(params::ID).unwrap_or_default();
        let affects_layout = id.starts_with("reader.") && id != crate::settings::OPEN_LAST.as_str();
        if *event == events::SETTING_CHANGED && affects_layout {
            self.selected = None;
            let env = *cx.env();
            book.relayout(&env);
            cx.request_redraw();
            let now = env.now;
            cx.wake_at(now);
        }
    }
}

/// Draws an image at its place on the page.
fn draw_image(canvas: &mut Canvas<'_>, env: &Env<'_>, t: &Typography, image: &PageImage) {
    let left = (t.x + image.x + 0.5) as i32;
    let top = (t.y + image.y + 0.5) as i32;
    let (w, h) = ((image.width + 0.5) as i32, (image.height + 0.5) as i32);
    quireink_sdk::ui::draw_image(
        canvas,
        env.storage,
        &image.image,
        Rect::new(left as u16, top as u16, w as u16, h as u16),
    );
}

/// The font bound to an optional reading face, for the layout key.
fn face_id(env: &Env<'_>, role: &quireink_sdk::Id) -> String {
    env.fonts
        .bound(role.as_str())
        .map_or_else(String::new, |font| font.id().to_string())
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
