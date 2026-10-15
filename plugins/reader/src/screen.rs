use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, fonts, params, slots};
use quireink_sdk::document::{Document, DocumentError, Link, Locator};
use quireink_sdk::layout::{Face, Hyphenate, Page, PageImage, Style, Typesetter, layout_page};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, FontHandle, FontSet, Gesture, GestureKind, Handled, Id,
    Params, Point, Rect, RefreshMode, Screen,
};

use crate::annotations::{self, Annotation, Kind, Mark};
use crate::index::PageIndex;
use crate::selection::{self, Item, Mode, Selection};
use crate::settings::{
    FONT_SIZE, FONT_WEIGHT, HYPHENATE, INDENT, JUSTIFY, LINE_SPACING, MARGIN, WORD_SPACING,
};
use crate::status;
use crate::{PLUGIN, state_path};
use quireink_sdk::status::context as status_context;

/// Pages laid out per background step; small enough to keep input snappy.
const PAGES_PER_STEP: usize = 8;
/// Pause between background steps.
const STEP_DELAY_MS: u64 = 2;
/// Pages remembered for going back before the index covers them.
const HISTORY: usize = 64;
/// Places jumps were made from (links, contents, search, page numbers),
/// kept for *Back*.
const RETURNS: usize = 32;

/// Page geometry and text style, derived from settings and the screen.
/// Relative sizes of headings 1 to 6.
const HEADING_SCALE: [f32; 6] = [1.6, 1.4, 1.2, 1.1, 1.0, 1.0];

/// Glyphs of a book's embedded fonts kept at most.
const EMBEDDED_CACHE: usize = 256 * 1024;
/// Largest total size of a book's embedded fonts loaded.
const EMBEDDED_LIMIT: usize = 8 * 1024 * 1024;

/// The fonts a book embeds, loaded with the book.
struct Embedded {
    set: FontSet,
    /// Family, bold, italic and index in `set` of each font.
    faces: Vec<(u8, bool, bool, usize)>,
}

/// Whether a book's own fonts are used: the font chosen is the book's
/// (the default choice), and they render on this device.
fn uses_book_fonts(env: &Env<'_>) -> bool {
    use quireink_sdk::builtin::settings::{DEFAULT_FONT, READING_FONT};
    quireink_sdk::text::OUTLINES && env.settings.text(READING_FONT.as_str()) == DEFAULT_FONT
}

impl Embedded {
    /// The book's fonts, if the reader uses them (the font chosen is the
    /// book's own) and they render here.
    fn load(env: &Env<'_>, document: &dyn Document) -> Option<Embedded> {
        if !uses_book_fonts(env) {
            return None;
        }
        let fonts = document.fonts(env.storage);
        if fonts.is_empty() {
            return None;
        }
        let mut set = FontSet::new(EMBEDDED_CACHE);
        let mut faces = Vec::new();
        let mut total = 0;
        for (index, font) in fonts.iter().enumerate() {
            let Ok(data) = document.font_data(env.storage, index) else {
                continue;
            };
            total += data.len();
            if total > EMBEDDED_LIMIT {
                break;
            }
            let Ok(loaded) = quireink_sdk::Font::from_data(data, &[]) else {
                continue;
            };
            let id = Id::parse(&format!("reader.book-font-{index}")).ok()?;
            let at = set.add(id, loaded);
            faces.push((font.family, font.bold, font.italic, at));
        }
        (!faces.is_empty()).then_some(Embedded { set, faces })
    }

    /// The face of `family` closest to `bold` and `italic`.
    fn face(&self, family: u8, bold: bool, italic: bool) -> Option<FontHandle<'_>> {
        let of_family = || self.faces.iter().filter(|f| f.0 == family);
        let exact = of_family().find(|f| f.1 == bold && f.2 == italic);
        let regular = || of_family().find(|f| !f.1 && !f.2);
        exact
            .or_else(regular)
            .or_else(|| of_family().next())
            .and_then(|f| self.set.get(f.3))
    }
}

/// The reading fonts by face, for layout and drawing.
struct Faces<'a> {
    /// Regular, bold, italic and bold italic.
    fonts: [FontHandle<'a>; 4],
    weight: u8,
    word_spacing: f32,
    /// The book's own fonts.
    embedded: Option<&'a Embedded>,
    size: f32,
    ascent: f32,
    line_height: f32,
}

impl<'a> Faces<'a> {
    /// The reading faces; missing ones fall back to the nearest face, and
    /// a font the user chose for book text is not mixed with the default
    /// bold or italic faces.
    fn new(env: &Env<'a>, t: &Typography, embedded: Option<&'a Embedded>) -> Option<Self> {
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
            weight: t.weight,
            word_spacing: t.word_spacing,
            embedded,
            size: t.size,
            ascent: t.ascent,
            line_height: t.line_height,
        })
    }

    /// The book's font for text in an embedded family, else the reader's.
    fn font(&self, face: Face) -> FontHandle<'a> {
        let style = face.style;
        if style.family() != 0
            && let Some(font) = self
                .embedded
                .and_then(|e| e.face(style.family(), style.bold(), style.italic()))
        {
            return font;
        }
        self.fonts[usize::from(style.bits() & 3)]
    }

    fn size(&self, face: Face) -> f32 {
        match (face.size, face.heading) {
            // The book's size wins.
            (percent, _) if percent > 0 => self.size * f32::from(percent) / 100.0,
            (_, 0) => self.size,
            (_, level) => self.size * HEADING_SCALE[usize::from(level.clamp(1, 6)) - 1],
        }
    }
}

impl Faces<'_> {
    /// A space between words in `face`.
    fn space(&self, face: Face) -> f32 {
        self.font(face).advance(self.size(face), ' ') * self.word_spacing
    }

    /// Draws `text` in `face`, as heavy as the font weight setting asks.
    fn draw(&self, canvas: &mut Canvas<'_>, face: Face, x: f32, baseline: i32, text: &str) -> f32 {
        let size = self.size(face);
        // About a pixel per step at 48 px; at least one.
        let extra = match self.weight {
            0 => 0,
            w => (f32::from(w) * size / 48.0 + 0.5).max(1.0) as u16,
        };
        self.font(face)
            .draw_bolder(canvas, size, x, baseline, text, Color::BLACK, extra)
    }
}

impl Typesetter for Faces<'_> {
    fn advance(&self, face: Face, c: char) -> f32 {
        if c == ' ' {
            return self.space(face);
        }
        self.font(face).advance(self.size(face), c)
    }

    fn line(&self, face: Face) -> (f32, f32) {
        if face.heading == 0 {
            // Body text the book sets larger or smaller keeps the line
            // spacing chosen.
            let scale = match face.size {
                0 => 1.0,
                percent => f32::from(percent) / 100.0,
            };
            return (self.ascent * scale, self.line_height * scale);
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
    /// Font weight steps (0: as the font is).
    weight: u8,
    /// Spaces relative to the font's.
    word_spacing: f32,
    /// Body text lines: baseline from the top of a line, line height.
    ascent: f32,
    line_height: f32,
    style: Style,
    /// Content box.
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    /// Status lines above and below the content, if they show anything.
    top_status: Option<Rect>,
    status: Option<Rect>,
    /// Language whose hyphenation patterns are used, if hyphenating.
    hyphenation: Option<String>,
    /// What the text filters did when it was made.
    filters: String,
    /// Identifies this layout; part of the page index file name.
    key: String,
}

impl Typography {
    /// Typography for a book in `language`; a book of `whole_pages` is
    /// shown edge to edge, without margins or status lines.
    fn new(env: &Env<'_>, font: &FontHandle<'_>, language: &str, whole_pages: bool) -> Self {
        let settings = env.settings;
        let m = env.metrics;
        let size = m.pt(settings.int(FONT_SIZE.as_str()) as f32);
        let weight = settings.int(FONT_WEIGHT.as_str()).clamp(0, 4) as u8;
        let word_spacing = settings.int(WORD_SPACING.as_str()).clamp(50, 200) as f32 / 100.0;
        let metrics = font.line_metrics(size);
        let natural = metrics.ascent + metrics.descent;
        let line_height = (size * settings.int(LINE_SPACING.as_str()) as f32 / 100.0).max(natural);
        let indent = settings.bool(INDENT.as_str());
        let justify = settings.bool(JUSTIFY.as_str());
        let ascent = metrics.ascent + (line_height - natural) / 2.0;
        let style = Style {
            paragraph_spacing: if indent { 0.0 } else { line_height * 0.6 },
            indent: if indent { size * 1.5 } else { 0.0 },
            em: size,
            justify,
        };

        let display = env.device.display;
        let margin = if whole_pages {
            0.0
        } else {
            m.mm(settings.int(MARGIN.as_str()).max(0) as u32) as f32
        };
        let status_height = status::height(env);
        let (top, bottom) = if whole_pages {
            (false, false)
        } else {
            (status::shown(env, true), status::shown(env, false))
        };
        let lines = f32::from(u8::from(top) + u8::from(bottom));
        let width = (f32::from(display.width) - 2.0 * margin).max(1.0);
        // Text never runs up to a status line, also on pages it fills to
        // the last pixel.
        let status_gap = m.mm(2) as f32;
        let status_room = status_height + status_gap;
        let height = (f32::from(display.height) - 2.0 * margin - lines * status_room).max(1.0);
        let y = margin + if top { status_room } else { 0.0 };
        let line = |y: f32| Rect::new(margin as u16, y as u16, width as u16, status_height as u16);
        let top_status = top.then(|| line(margin));
        let status = bottom.then(|| line(y + height + status_gap));
        let hyphenation = (settings.bool(HYPHENATE.as_str())
            && env.hyphenation.get(language).is_some())
        .then(|| language.to_string());
        let key = format!(
            "{:016x}",
            fnv(&format!(
                "{}|{}|{}|{}|{}|{size}|{line_height}|{}|{}|{justify}|{width}x{height}|{}|{}|{}|{word_spacing}|{}|{whole_pages}",
                quireink_sdk::layout::VERSION,
                font.id(),
                face_id(env, &fonts::READING_BOLD),
                face_id(env, &fonts::READING_ITALIC),
                face_id(env, &fonts::READING_BOLD_ITALIC),
                style.indent,
                style.paragraph_spacing,
                hyphenation.as_deref().unwrap_or("-"),
                settings.bool(crate::settings::PUBLISHER_STYLES.as_str()),
                uses_book_fonts(env),
                env.text_filters.signature(env),
            ))
        );
        Self {
            size,
            weight,
            word_spacing,
            ascent,
            line_height,
            style,
            x: margin,
            y,
            width,
            height,
            top_status,
            status,
            hyphenation,
            filters: env.text_filters.signature(env),
            key,
        }
    }
}

/// The book as the layout sees it: every block through the text
/// filters of plugins (see `quireink_sdk::filters`).
struct Filtered<'d, 'e> {
    document: &'d dyn Document,
    env: &'d Env<'e>,
}

impl Document for Filtered<'_, '_> {
    fn metadata(&self) -> &quireink_sdk::document::Metadata {
        self.document.metadata()
    }

    fn sections(&self) -> u32 {
        self.document.sections()
    }

    fn section_len(&self, section: u32) -> u32 {
        self.document.section_len(section)
    }

    fn block_at(
        &self,
        storage: &dyn quireink_sdk::Storage,
        at: Locator,
    ) -> Result<Option<quireink_sdk::document::Block>, DocumentError> {
        let mut block = self.document.block_at(storage, at)?;
        if let Some(block) = &mut block {
            self.env.text_filters.apply(self.env, block);
        }
        Ok(block)
    }

    fn progress(&self, at: Locator) -> f32 {
        self.document.progress(at)
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
    /// Where the bookmarks and highlights are.
    marks: Vec<Mark>,
    /// Text to point out on this page, e.g. a search hit.
    found: Option<(Locator, Locator)>,
    /// The chapter of the page, for status lines: where it starts, where
    /// the next one starts and its title (empty before the first).
    chapter: Option<(Locator, Option<Locator>, String)>,
    /// The book's own fonts, if used.
    embedded: Option<Embedded>,
    /// The links on the page.
    links: Vec<Link>,
    /// Where links were followed from, the last last.
    returns: Vec<Locator>,
    /// The status lines chosen last, top then bottom.
    status_lines: [String; 2],
    /// Where the top-level chapters begin, as thousandths of the book
    /// (see `status_context::CHAPTERS`).
    chapter_marks: String,
    /// The page the last link led to: it offers the way back.
    landed: Option<Locator>,
    /// Read aloud (`volume`) or turning its pages by itself (`clock`):
    /// the icon shown in the status line.
    hands_free: Option<&'static str>,
}

/// A word on the page, with its box on the screen and its place in the
/// book.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WordBox {
    pub area: Rect,
    pub text: String,
    pub start: Locator,
    pub end: Locator,
}

impl WordBox {
    /// The word without surrounding punctuation, as looked up.
    fn lookup_text(&self) -> &str {
        self.text.trim_matches(|c: char| !c.is_alphanumeric())
    }
}

/// The data folder of the book at `path`.
/// Remembers `path` as the book read last (reopened at start-up).
pub(crate) fn remember_last(storage: &dyn quireink_sdk::Storage, path: &str) {
    let state = format!(
        "[reader]
last = \"{}\"
",
        escape(path)
    );
    let _ = storage.write(&state_path(), state.as_bytes());
}

pub(crate) fn book_dir(path: &str) -> String {
    format!(
        "{}/books/{:016x}",
        Env::plugin_dir(PLUGIN.as_str()),
        fnv(path)
    )
}

/// Shows a book page by page.
/// Choosing a link with the buttons: which of the page's links that can
/// be followed has the focus (`None` if the page has none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Linking {
    focus: Option<usize>,
}

pub(crate) struct ReaderScreen {
    book: Result<Book, String>,
    /// Text chosen on the page (see `selection`).
    selection: Option<Selection>,
    /// A link being chosen with the buttons.
    linking: Option<Linking>,
    /// The last search, offered again.
    last_search: String,
    /// What the status lines showed when last drawn.
    status_shown: String,
    /// The sentence being read aloud.
    aloud: Option<Aloud>,
    /// Where reading aloud was paused: the page and its word.
    resume: Option<(Locator, usize)>,
    /// When the page turns by itself next, while it does.
    auto_turn: Option<quireink_sdk::Instant>,
    /// Where skimming began, while skimming.
    skim_from: Option<Locator>,
}

/// A sentence read aloud, by words of the page that starts at `page`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sentence {
    page: Locator,
    from: usize,
    to: usize,
    /// The words of its page.
    words: usize,
    /// Words of the following page spoken with it (it goes on there).
    carried: usize,
}

/// Reading aloud: the sentence spoken and the one handed over to follow
/// it without a pause.
#[derive(Debug, Clone, Copy)]
struct Aloud {
    now: Sentence,
    next: Option<Sentence>,
    /// When the timer stops reading aloud.
    until: Option<quireink_sdk::Instant>,
}

/// Where the underline of words `range` goes, if any is on the page.
fn aloud_area(env: &Env<'_>, words: &[WordBox], range: core::ops::Range<usize>) -> Option<Rect> {
    let line = env.metrics.tenth_mm(5).max(2) as u16;
    selection::line_boxes(words, |i| range.contains(&i))
        .into_iter()
        .map(|b| Rect::new(b.x, b.y, b.width, b.height + 2 * line))
        .reduce(|a, b| a.union(&b))
}

impl ReaderScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        Self {
            book: Book::open(env, params),
            selection: None,
            linking: None,
            last_search: String::new(),
            status_shown: String::new(),
            aloud: None,
            resume: None,
            auto_turn: None,
            skim_from: None,
        }
    }

    /// Opens the skimming overlay where the book is.
    fn open_skim(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return;
        };
        self.skim_from.get_or_insert(book.page.start);
        cx.command(Command::OpenScreen(
            crate::SKIM_SCREEN,
            book.skim_params(&env)
                .with(crate::skim::MARKS, book.chapter_marks.clone()),
        ));
    }

    /// Moves as the skimming overlay asks, and tells it where the book is.
    fn skim(&mut self, to: &str, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let from = self.skim_from;
        let Ok(book) = &mut self.book else {
            return;
        };
        let target = if to == "back" {
            from
        } else {
            book.skim_target(&env, to)
        };
        if let Some(target) = target {
            let start = book.page_start(&env, target);
            book.show(cx, start, true);
        }
        cx.command(Command::Emit(crate::SKIMMED, book.skim_params(&env)));
    }

    /// Starts turning the pages by themselves, or stops it.
    fn toggle_auto_turn(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if self.auto_turn.take().is_some() {
            cx.command(Command::Notify(
                env.i18n.tr("reader.auto-turn.stopped").to_string(),
            ));
            self.hands_free_shown(cx);
            return;
        }
        let seconds = self.auto_turn_seconds(&env);
        self.auto_turn = Some(env.now.add_millis(seconds * 1000));
        cx.command(Command::Notify(env.i18n.format(
            "reader.auto-turn.started",
            &[("seconds", Arg::Int(seconds as i64))],
        )));
        self.hands_free_shown(cx);
        self.update_status(cx);
    }

    fn auto_turn_seconds(&self, env: &Env<'_>) -> u64 {
        env.settings
            .int(crate::settings::AUTO_TURN_SECONDS.as_str())
            .clamp(5, 600) as u64
    }

    /// Turns the page if it is time; stops at the end of the book.
    fn auto_turn_due(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if !self.auto_turn.is_some_and(|at| env.now >= at) {
            return;
        }
        let seconds = self.auto_turn_seconds(&env);
        let Ok(book) = &mut self.book else {
            return;
        };
        let end = book.page.end;
        self.auto_turn = book
            .show(cx, end, true)
            .then(|| env.now.add_millis(seconds * 1000));
        if self.auto_turn.is_none() {
            self.hands_free_shown(cx);
        }
    }

    /// Starts reading aloud on the page shown, where it was paused if
    /// that was on this page, where the device can.
    fn start_aloud(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        if !env.speech {
            cx.command(Command::Notify(
                env.i18n.tr("reader.aloud.unavailable").to_string(),
            ));
            return;
        }
        let Ok(book) = &self.book else {
            return;
        };
        let from = match self.resume.take() {
            Some((page, word)) if page == book.page.start => word,
            _ => 0,
        };
        let until = crate::aloud::timer_minutes(&env).map(|m| env.now.add_millis(m * 60_000));
        cx.command(Command::Notify(
            env.i18n.tr("reader.aloud.started").to_string(),
        ));
        self.begin_aloud(from, until, cx);
    }

    /// Speaks from word `from` of the page shown, handing over the next
    /// sentence at once.
    fn begin_aloud(
        &mut self,
        from: usize,
        until: Option<quireink_sdk::Instant>,
        cx: &mut Cx<'_, '_>,
    ) {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return;
        };
        let turn = env.settings.bool(crate::settings::ALOUD_TURN.as_str());
        let (now, text) = book.sentence(&env, &book.page, from, turn);
        book.speak(&env, text, false, cx);
        let next = book.following(&env, &now, turn).map(|(next, text)| {
            book.speak(&env, text, true, cx);
            next
        });
        let before = self.aloud.map(|a| a.now);
        self.aloud = Some(Aloud { now, next, until });
        self.underline(before, Some(now), cx);
        self.hands_free_shown(cx);
    }

    /// Shows in the status line whether the book is being read aloud.
    fn hands_free_shown(&mut self, cx: &mut Cx<'_, '_>) {
        let icon = if self.aloud.is_some() {
            Some("volume")
        } else if self.auto_turn.is_some() {
            Some("clock")
        } else {
            None
        };
        if let Ok(book) = &mut self.book
            && book.hands_free != icon
        {
            book.hands_free = icon;
            self.update_status(cx);
        }
    }

    /// Pauses reading aloud; reading aloud again goes on from there.
    fn pause_aloud(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(aloud) = self.aloud.take() else {
            return;
        };
        cx.command(Command::StopSpeaking);
        self.resume = Some((aloud.now.page, aloud.now.from));
        self.underline(Some(aloud.now), None, cx);
        self.hands_free_shown(cx);
        let env = *cx.env();
        cx.command(Command::Notify(
            env.i18n.tr("reader.aloud.paused").to_string(),
        ));
    }

    /// Moves the underline from sentence `before` to `now`, where they
    /// are on the page shown.
    fn underline(&self, before: Option<Sentence>, now: Option<Sentence>, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return;
        };
        if !env.settings.bool(crate::settings::ALOUD_HIGHLIGHT.as_str()) {
            return;
        }
        let words = book.words(&env);
        let area = [before, now]
            .into_iter()
            .flatten()
            .filter(|s| s.page == book.page.start)
            .filter_map(|s| aloud_area(&env, &words, s.from..s.to))
            .reduce(|a, b| a.union(&b));
        if let Some(area) = area {
            cx.request_redraw_area(area);
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    /// A sentence was read aloud: the next one is being spoken already;
    /// the one after it is handed over, turning the page where needed.
    /// Ends at the end of the book, of the page if pages are not turned,
    /// or with the timer.
    fn spoken(&mut self, params: &Params, cx: &mut Cx<'_, '_>) {
        let Some(aloud) = self.aloud else {
            return;
        };
        let env = *cx.env();
        if params.get(quireink_sdk::speech_params::FAILED).is_some() {
            self.pause_aloud(cx);
            self.resume = None;
            cx.command(Command::Notify(
                env.i18n.tr("reader.aloud.failed").to_string(),
            ));
            return;
        }
        if aloud.until.is_some_and(|until| env.now >= until) {
            self.pause_aloud(cx);
            cx.command(Command::Notify(
                env.i18n.tr("reader.aloud.timer-done").to_string(),
            ));
            return;
        }
        let turn = env.settings.bool(crate::settings::ALOUD_TURN.as_str());
        let Some(next) = aloud.next else {
            // The end of the book, or of the page.
            self.aloud = None;
            self.underline(Some(aloud.now), None, cx);
            self.hands_free_shown(cx);
            if !turn {
                cx.command(Command::Notify(
                    env.i18n.tr("reader.aloud.page-done").to_string(),
                ));
            }
            return;
        };
        let Ok(book) = &mut self.book else {
            return;
        };
        if next.page != book.page.start {
            let end = book.page.end;
            book.show(cx, end, true);
            if book.page.start != next.page {
                // Laid out anew meanwhile: on from the page's start.
                cx.command(Command::StopSpeaking);
                return self.begin_aloud(0, aloud.until, cx);
            }
        }
        let after = book.following(&env, &next, turn).map(|(after, text)| {
            book.speak(&env, text, true, cx);
            after
        });
        self.aloud = Some(Aloud {
            now: next,
            next: after,
            until: aloud.until,
        });
        self.underline(Some(aloud.now), Some(next), cx);
    }

    /// The bar's items for the current selection.
    fn items(&self, env: &Env<'_>, words: &[WordBox]) -> Vec<Item> {
        // The highlight under the selection, if any, and whether it has a
        // note.
        let highlight = match (&self.book, self.selection) {
            (Ok(book), Some(selection)) => {
                let (lo, hi) = selection.range();
                words.get(lo).zip(words.get(hi)).and_then(|(a, b)| {
                    book.marks
                        .iter()
                        .find(|m| m.kind == Kind::Highlight && m.overlaps(a.start, b.end))
                        .map(|m| m.noted)
                })
            }
            _ => None,
        };
        selection::items(env, highlight)
    }

    /// Changes the selection and redraws what it covered and covers.
    fn set_selection(&mut self, to: Option<Selection>, cx: &mut Cx<'_, '_>) {
        let bar_before = self.selection.is_some_and(|s| s.mode == Mode::Bar);
        let bar_after = to.is_some_and(|s| s.mode == Mode::Bar);
        self.selection = to;
        // Fast refreshes are black and white: they would hide the gray of
        // highlights.
        let highlights = self.book.as_ref().is_ok_and(|book| {
            book.marks
                .iter()
                .any(|m| m.kind == Kind::Highlight && m.overlaps(book.page.start, book.page.end))
        });
        cx.request_redraw();
        cx.request_refresh(if bar_before != bar_after || highlights {
            RefreshMode::Partial
        } else {
            RefreshMode::Fast
        });
    }

    /// Does what a bar item stands for. Items of other plugins are
    /// announced with the selected text (event [`crate::TEXT_CHOSEN`]).
    fn activate(&mut self, item: &Id, cx: &mut Cx<'_, '_>) {
        if *item == crate::actions::READ_FROM
            && let (Ok(book), Some(selection)) = (&self.book, self.selection)
        {
            // From the first word chosen.
            self.resume = Some((book.page.start, selection.range().0));
            self.set_selection(None, cx);
            self.start_aloud(cx);
            return;
        }
        let env = *cx.env();
        let (Ok(book), Some(selection)) = (&mut self.book, self.selection) else {
            return;
        };
        let words = book.words(&env);
        let (lo, hi) = selection.range();
        let (Some(first), Some(last)) = (words.get(lo), words.get(hi)) else {
            self.set_selection(None, cx);
            return;
        };
        let (start, end) = (first.start, last.end);
        let text = words[lo..=hi]
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if *item == crate::actions::LOOK_UP {
            let phrase = text.trim_matches(|c: char| !c.is_alphanumeric());
            if !phrase.is_empty() {
                cx.command(Command::OpenSlot(
                    slots::LOOKUP,
                    Params::new()
                        .with(params::WORD, phrase.to_string())
                        .with(params::LANGUAGE, book.language.clone()),
                ));
            }
            return;
        }
        if *item == crate::actions::MORE {
            // The whole selection menu as a list; the chosen action comes
            // back to `on_action` while the text stays selected.
            cx.command(Command::OpenSlot(
                slots::MENU,
                Params::new().with(params::MENU, crate::SELECTION_MENU.as_str()),
            ));
            return;
        }
        if *item == crate::actions::EXTEND {
            let to = Selection {
                mode: Mode::Extending,
                focus: None,
                ..selection
            };
            self.set_selection(Some(to), cx);
            return;
        }
        if *item == crate::actions::COPY {
            cx.command(Command::SetClipboard(text.clone()));
        } else if *item == crate::actions::HIGHLIGHT {
            let highlight = |m: &Mark| m.kind == Kind::Highlight && m.overlaps(start, end);
            // On highlighted text the item removes the highlight.
            if !annotations::remove(env.storage, &book.dir, highlight) {
                let annotation = Annotation {
                    mark: Mark {
                        kind: Kind::Highlight,
                        start,
                        end,
                        noted: false,
                    },
                    percent: (book.document.progress(start) * 100.0 + 0.5) as u8,
                    text: annotations::snippet(&text),
                    note: String::new(),
                };
                annotations::add(env.storage, &book.dir, annotation);
            }
            book.marks = annotations::marks(env.storage, &book.dir);
        } else if *item == crate::actions::NOTE {
            // On the highlight there (made if there is none), typed on
            // the keyboard.
            let existing = book
                .marks
                .iter()
                .find(|m| m.kind == Kind::Highlight && m.overlaps(start, end))
                .map(|m| m.start);
            let at = existing.unwrap_or_else(|| {
                annotations::add(
                    env.storage,
                    &book.dir,
                    Annotation {
                        mark: Mark {
                            kind: Kind::Highlight,
                            start,
                            end,
                            noted: false,
                        },
                        percent: (book.document.progress(start) * 100.0 + 0.5) as u8,
                        text: annotations::snippet(&text),
                        note: String::new(),
                    },
                );
                start
            });
            book.marks = annotations::marks(env.storage, &book.dir);
            let note = annotations::note_of(env.storage, &book.dir, at);
            cx.command(Command::OpenSlot(
                slots::KEYBOARD,
                crate::toc::locator_params(&book.path, at)
                    .with(params::TITLE, env.i18n.tr("reader.note.prompt").to_string())
                    .with(params::TEXT, note)
                    .with(params::REPLY, crate::NOTE_ENTERED.as_str()),
            ));
        } else if *item != crate::actions::CANCEL {
            cx.command(Command::Emit(
                crate::TEXT_CHOSEN,
                crate::toc::locator_params(&book.path, start)
                    .with(crate::ACTION, item.as_str().to_string())
                    .with(crate::TEXT, text)
                    .with(params::LANGUAGE, book.language.clone())
                    .with(crate::END_SECTION, format!("{}", end.section))
                    .with(crate::END_OFFSET, format!("{}", end.offset)),
            ));
            // Then the action runs as usual (after the selection is gone),
            // so any action works here, e.g. the night mode.
            cx.command(Command::RunAction(item.clone()));
        }
        self.set_selection(None, cx);
        cx.request_refresh(RefreshMode::Partial);
    }

    /// Buttons while text is selected.
    /// Shows the next status line at the top (or bottom) and keeps the
    /// choice.
    fn next_status_line(&mut self, top: bool, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &mut self.book else {
            return;
        };
        let slot = usize::from(!top);
        let next = status::next(&env, top, &book.status_lines[slot]).to_string();
        if next == status::current(&env, top, &book.status_lines[slot]) {
            return;
        }
        book.status_lines[slot] = next;
        status::save_choice(env.storage, &book.status_lines);
        for (area, at_top) in book.status_areas() {
            if at_top == top {
                cx.request_redraw_area(area);
            }
        }
        cx.request_refresh(RefreshMode::Fast);
        self.status_shown.clear();
    }

    fn set_linking(&mut self, to: Option<Linking>, cx: &mut Cx<'_, '_>) {
        if self.linking != to {
            self.linking = to;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    /// Buttons while choosing a link: the next or previous one (on to
    /// the next or previous page past the last or first), *confirm*
    /// follows it, *back* ends.
    fn on_linking(&mut self, action: &Id, linking: Linking, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let a = action;
        let forward = [
            actions::RIGHT,
            actions::DOWN,
            actions::NEXT,
            actions::NEXT_PAGE,
        ];
        let backward = [
            actions::LEFT,
            actions::UP,
            actions::PREVIOUS,
            actions::PREV_PAGE,
        ];
        let Ok(book) = &mut self.book else {
            self.linking = None;
            return Handled::No;
        };
        let count = book.followable(&env).len();
        if forward.contains(a) {
            match linking.focus {
                Some(i) if i + 1 < count => {
                    self.set_linking(Some(Linking { focus: Some(i + 1) }), cx);
                }
                _ => {
                    let end = book.page.end;
                    book.show(cx, end, true);
                    let focus = (!book.followable(&env).is_empty()).then_some(0);
                    self.linking = Some(Linking { focus });
                    cx.request_redraw();
                }
            }
        } else if backward.contains(a) {
            match linking.focus {
                Some(i) if i > 0 => {
                    self.set_linking(Some(Linking { focus: Some(i - 1) }), cx);
                }
                _ => {
                    if let Some(start) = book.previous_start(&env) {
                        book.show(cx, start, false);
                    }
                    let count = book.followable(&env).len();
                    self.linking = Some(Linking {
                        focus: count.checked_sub(1),
                    });
                    cx.request_redraw();
                }
            }
        } else if *a == actions::CONFIRM {
            let link = linking
                .focus
                .and_then(|i| book.followable(&env).get(i).copied())
                .map(|i| book.links[i].clone());
            let link = link.map(|l| {
                let words = book.words(&env);
                (l.href.clone(), link_label(book, &words, &l))
            });
            self.set_linking(None, cx);
            if let Some((href, label)) = link {
                return self.open_link(&href, &label, cx);
            }
        } else if *a == actions::BACK || *a == actions::MENU {
            self.set_linking(None, cx);
        } else {
            // Anything else ends choosing and goes on as usual.
            self.set_linking(None, cx);
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_selection(&mut self, action: &Id, selection: Selection, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return Handled::No;
        };
        let words = book.words(&env);
        if words.is_empty() {
            self.set_selection(None, cx);
            return Handled::No;
        }
        let a = action;
        if *a == actions::BACK || *a == actions::MENU {
            self.set_selection(None, cx);
            return Handled::Yes;
        }
        // An action of the selection menu, e.g. chosen in its list.
        if env
            .menus
            .items(crate::SELECTION_MENU.as_str())
            .iter()
            .any(|item| item.id == a)
        {
            self.activate(a, cx);
            return Handled::Yes;
        }
        match selection.mode {
            Mode::Choosing | Mode::Extending => {
                if *a == actions::CONFIRM {
                    let to = Selection {
                        mode: Mode::Bar,
                        focus: Some(0),
                        ..selection
                    };
                    self.set_selection(Some(to), cx);
                } else if let Some(cursor) = selection.moved(a, &words) {
                    let anchor = if selection.mode == Mode::Choosing {
                        cursor
                    } else {
                        selection.anchor
                    };
                    let to = Selection {
                        anchor,
                        cursor,
                        ..selection
                    };
                    self.set_selection(Some(to), cx);
                } else {
                    return Handled::No;
                }
            }
            Mode::Bar => {
                let items = self.items(&env, &words);
                let focus = selection.focus.unwrap_or(0);
                if *a == actions::CONFIRM {
                    if let Some(item) = items.get(focus) {
                        self.activate(&item.id.clone(), cx);
                    }
                } else if [
                    actions::LEFT,
                    actions::UP,
                    actions::PREVIOUS,
                    actions::PREV_PAGE,
                ]
                .contains(a)
                {
                    let to = Selection {
                        focus: Some(focus.saturating_sub(1)),
                        ..selection
                    };
                    self.set_selection(Some(to), cx);
                } else if [
                    actions::RIGHT,
                    actions::DOWN,
                    actions::NEXT,
                    actions::NEXT_PAGE,
                ]
                .contains(a)
                {
                    let to = Selection {
                        focus: Some((focus + 1).min(items.len() - 1)),
                        ..selection
                    };
                    self.set_selection(Some(to), cx);
                } else {
                    return Handled::No;
                }
            }
        }
        Handled::Yes
    }

    /// Redraws the status lines if what they show changed, and checks again
    /// when the minute changes.
    fn update_status(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return;
        };
        if book.status_areas().next().is_none() {
            return;
        }
        let signature = status::signature(&env, &book.status_context(&env), &book.status_lines);
        if signature != self.status_shown {
            self.status_shown = signature;
            for (area, _) in book.status_areas() {
                cx.request_redraw_area(area);
            }
            cx.request_refresh(RefreshMode::Fast);
        }
        let next = quireink_sdk::time::next_minute(&env).unwrap_or(env.now.add_millis(60_000));
        cx.wake_at(next);
        if let Some(at) = self.auto_turn {
            cx.wake_at(at);
        }
    }

    /// Asks on the keyboard what to search for, or which page to go to.
    fn ask(&mut self, action: &Id, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &self.book else {
            return;
        };
        let request = Params::new().with(params::PATH, book.path.clone());
        let request = if *action == crate::actions::SEARCH {
            request
                .with(
                    params::TITLE,
                    env.i18n.tr("reader.search.prompt").to_string(),
                )
                .with(params::TEXT, self.last_search.clone())
                .with(params::REPLY, crate::SEARCH_ENTERED.as_str())
        } else {
            let (pages, complete) = book
                .index
                .as_ref()
                .map_or((0, false), |i| (i.pages(), i.is_complete()));
            let key = if complete {
                "reader.go-to-page.prompt"
            } else {
                "reader.go-to-page.counting"
            };
            let title = env
                .i18n
                .format(key, &[("pages", Arg::Int(i64::from(pages)))]);
            request
                .with(params::TITLE, title)
                .with(params::MODE, "number")
                .with(params::REPLY, crate::PAGE_ENTERED.as_str())
        };
        cx.command(Command::OpenSlot(slots::KEYBOARD, request));
    }

    /// Opens a link: a note over the page if it looks like one and the
    /// reader shows notes that way, else follows it.
    fn open_link(&mut self, href: &str, label: &str, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let popup = env.settings.text(crate::settings::FOOTNOTES.as_str())
            == crate::settings::FOOTNOTES_POPUP;
        if popup
            && !quireink_sdk::document::is_external(href)
            && crate::footnote::note_like(label)
            && let Ok(book) = &self.book
            && let Some(text) = book.note_text(&env, href)
        {
            cx.command(Command::OpenScreen(
                crate::FOOTNOTE_SCREEN,
                Params::new()
                    .with(params::TEXT, text)
                    .with(params::PATH, book.path.clone())
                    .with(crate::footnote::HREF, href.to_string()),
            ));
            return Handled::Yes;
        }
        self.follow(href, cx)
    }

    /// Follows link `href`: to its place in the book, remembering where
    /// it was followed from for *Back*.
    fn follow(&mut self, href: &str, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let Ok(book) = &mut self.book else {
            return Handled::No;
        };
        if quireink_sdk::document::is_external(href) {
            // To the web, if a browser is there.
            if !env.has_slot(&slots::BROWSER) {
                return Handled::No;
            }
            // Told apart by the time, so a page asked for again is loaded
            // again rather than the open browser just brought forward.
            cx.command(Command::OpenSlot(
                slots::BROWSER,
                Params::new()
                    .with(params::URL, href.to_string())
                    .with(params::ID, format!("{}", env.now.0)),
            ));
            return Handled::Yes;
        }
        let Some(target) = book.document.link_target(env.storage, href) else {
            return Handled::No;
        };
        let start = book.page_start(&env, target);
        book.jump(cx, start);
        Handled::Yes
    }

    /// Sets or removes the bookmark of the page.
    fn toggle_bookmark(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &mut self.book else {
            return;
        };
        let (start, end) = (book.page.start, book.page.end);
        let on_page = |m: &Mark| m.kind == Kind::Bookmark && m.start >= start && m.start < end;
        if !annotations::remove(env.storage, &book.dir, on_page) {
            let text: Vec<&str> = book.page.lines.iter().map(|l| l.text.as_str()).collect();
            annotations::add(
                env.storage,
                &book.dir,
                Annotation {
                    mark: Mark {
                        kind: Kind::Bookmark,
                        start,
                        end: start,
                        noted: false,
                    },
                    percent: (book.percent() + 0.5) as u8,
                    text: annotations::snippet(&text.join(" ")),
                    note: String::new(),
                },
            );
        }
        book.marks = annotations::marks(env.storage, &book.dir);
        cx.request_redraw_area(book.ribbon_area(&env));
        cx.request_refresh(RefreshMode::Fast);
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
        let styles = env
            .settings
            .bool(crate::settings::PUBLISHER_STYLES.as_str());
        let options = quireink_sdk::OpenOptions::default().publisher_styles(styles);
        let document = env
            .formats
            .open_with(env.storage, path, &options)
            .map_err(error)?;
        let font = env
            .fonts
            .role(fonts::READING.as_str())
            .ok_or_else(|| env.i18n.tr("reader.no-font").to_string())?;
        let language = document
            .metadata()
            .language
            .clone()
            .unwrap_or_else(|| env.i18n.language().to_string());
        let typography = Typography::new(env, &font, &language, document.whole_pages());
        let dir = book_dir(path);
        let marks = annotations::marks(env.storage, &dir);
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
            marks,
            found: None,
            chapter: None,
            embedded: None,
            links: Vec::new(),
            returns: Vec::new(),
            status_lines: status::load_choice(env.storage),
            chapter_marks: String::new(),
            landed: None,
            hands_free: None,
        };
        book.chapter_marks = book.chapter_marks(env);
        book.embedded = Embedded::load(env, book.document.as_ref());
        let start = book.saved_position(env).unwrap_or(Locator::START);
        book.page = match book.layout(env, start).map_err(error)? {
            page if page.is_past_end() && start != Locator::START => {
                book.layout(env, Locator::START).map_err(error)?
            }
            page => page,
        };
        book.index = Some(book.open_index(env));
        book.update_chapter(env);
        book.update_links(env);
        if !book.web() {
            remember_last(env.storage, &book.path);
        }
        Ok(book)
    }

    /// Parameters of the standard reading events; the page and the number
    /// of pages once they are counted.
    fn event_params(&self, env: &Env<'_>) -> Params {
        let mut params = Params::new()
            .with(params::PATH, self.path.clone())
            .with(params::TITLE, self.title.clone())
            .with(params::PROGRESS, format!("{:.1}", self.percent()))
            .with(params::SECTION, format!("{}", self.page.start.section))
            .with(params::OFFSET, format!("{}", self.page.start.offset));
        if let Some(index) = self.index.as_ref().filter(|i| i.is_complete())
            && let Some((page, true)) = index.locate(env.storage, self.page.start)
        {
            params = params
                .with(params::PAGE, format!("{}", page + 1))
                .with(params::PAGES, format!("{}", index.pages()));
        }
        params
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
        self.typography = Typography::new(env, &font, &self.language, self.document.whole_pages());
        if let Ok(page) = self.layout(env, self.page.start) {
            self.page = page;
        }
        self.update_links(env);
        self.history.clear();
        self.index = Some(self.open_index(env));
        self.indexing = None;
    }

    fn font<'a>(&self, env: &Env<'a>) -> Option<FontHandle<'a>> {
        env.fonts.role(fonts::READING.as_str())
    }

    fn layout(&self, env: &Env<'_>, at: Locator) -> Result<Page, DocumentError> {
        let t = &self.typography;
        let Some(faces) = Faces::new(env, t, self.embedded.as_ref()) else {
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
        let filtered = Filtered {
            document: self.document.as_ref(),
            env,
        };
        layout_page(
            &filtered,
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
        self.found = None;
        if forward {
            if self.history.len() == HISTORY {
                self.history.remove(0);
            }
            self.history.push(self.page.start);
        }
        self.page = page;
        self.update_chapter(&env);
        self.update_links(&env);
        self.save_position(&env);
        if !self.web() {
            cx.command(Command::Emit(events::PAGE_TURNED, self.event_params(&env)));
        }
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
        let Some(faces) = Faces::new(env, t, self.embedded.as_ref()) else {
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
                let text = &line.text[run.start..run.end];
                let space = faces.space(run.face) + line.word_spacing;
                for (i, word) in text.split(' ').enumerate() {
                    if i > 0 {
                        x += space;
                    }
                    x += faces.draw(canvas, run.face, x, baseline, word);
                }
            }
        }
    }

    /// The sentence that starts at word `from` of `page` (empty past its
    /// last word) and its text as spoken; with `turn`, one that goes on on
    /// the next page is spoken whole.
    fn sentence(&self, env: &Env<'_>, page: &Page, from: usize, turn: bool) -> (Sentence, String) {
        let words = self.words_of(env, page);
        let from = from.min(words.len());
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let to = if from < texts.len() {
            crate::aloud::sentence_end(&texts, from)
        } else {
            from
        };
        let starts_line =
            |words: &[WordBox], i: usize| i == 0 || words[i].area.y != words[i - 1].area.y;
        let mut parts: Vec<(&str, bool)> = (from..to)
            .map(|i| (texts[i], starts_line(&words, i)))
            .collect();
        // The rest of a sentence going on on the next page.
        let next_words;
        let mut carried = 0;
        let unfinished =
            to > from && to == words.len() && !crate::aloud::ends_sentence(texts[to - 1]);
        if unfinished
            && turn
            && let Ok(next) = self.layout(env, page.end)
            && !next.is_past_end()
        {
            next_words = self.words_of(env, &next);
            let next_texts: Vec<&str> = next_words.iter().map(|w| w.text.as_str()).collect();
            carried = crate::aloud::sentence_end(&next_texts, 0);
            parts.extend((0..carried).map(|i| (next_texts[i], starts_line(&next_words, i))));
        }
        let sentence = Sentence {
            page: page.start,
            from,
            to,
            words: words.len(),
            carried,
        };
        (sentence, crate::aloud::spoken(parts.into_iter()))
    }

    /// The sentence after `sentence` with its text: on its page, else
    /// (with `turn`) on the next one; `None` at the end.
    fn following(
        &self,
        env: &Env<'_>,
        sentence: &Sentence,
        turn: bool,
    ) -> Option<(Sentence, String)> {
        let laid_out;
        let page = if sentence.page == self.page.start {
            &self.page
        } else {
            laid_out = self.layout(env, sentence.page).ok()?;
            &laid_out
        };
        if sentence.to < sentence.words {
            return Some(self.sentence(env, page, sentence.to, turn));
        }
        if !turn {
            return None;
        }
        let next = self.layout(env, page.end).ok()?;
        if next.is_past_end() || next.start == page.start {
            return None;
        }
        Some(self.sentence(env, &next, sentence.carried, turn))
    }

    /// Hands `text` over to be spoken, after what is spoken with `queue`.
    fn speak(&self, env: &Env<'_>, text: String, queue: bool, cx: &mut Cx<'_, '_>) {
        let mut speech = quireink_sdk::Speech::new(text, crate::SPOKEN);
        speech.language = self.language.clone();
        speech.rate = env
            .settings
            .int(crate::settings::ALOUD_SPEED.as_str())
            .clamp(50, 300) as u16;
        speech.params = Params::new().with(params::PATH, self.path.clone());
        speech.queue = queue;
        cx.command(Command::Speak(speech));
    }

    /// The words of the page with their boxes on the screen, in reading
    /// order, measured as they are drawn.
    fn words(&self, env: &Env<'_>) -> Vec<WordBox> {
        self.words_of(env, &self.page)
    }

    /// The words of `page`, as [`Book::words`].
    fn words_of(&self, env: &Env<'_>, page: &Page) -> Vec<WordBox> {
        let t = &self.typography;
        let Some(faces) = Faces::new(env, t, self.embedded.as_ref()) else {
            return Vec::new();
        };
        let mut words: Vec<WordBox> = Vec::new();
        for line in &page.lines {
            let mut x = t.x + line.x;
            // Inside a word that continues in the next run (e.g. bold).
            let mut joined = false;
            // The next of the line's words.
            let mut ordinal = 0;
            for run in &line.runs {
                let font = faces.font(run.face);
                let size = faces.size(run.face);
                let (ascent, height) = faces.line(run.face);
                let top = t.y + line.baseline - ascent;
                let space = faces.space(run.face) + line.word_spacing;
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
                        _ => {
                            let (start, end) = line.words.get(ordinal).copied().unwrap_or_default();
                            ordinal += 1;
                            words.push(WordBox {
                                area: Rect::new(x as u16, top as u16, width as u16, height as u16),
                                text: part.to_string(),
                                start: Locator::new(line.section, start),
                                end: Locator::new(line.section, end),
                            });
                        }
                    }
                    x += width;
                    joined = true;
                }
            }
        }
        words
    }

    /// Whether this is no book but a document a plugin keeps for itself,
    /// e.g. a web page: it is not reported as read (see
    /// [`events::BOOK_OPENED`]) nor opened again at start-up.
    fn web(&self) -> bool {
        quireink_sdk::files::is_hidden(&self.path)
    }

    /// Reads the links of the page.
    fn update_links(&mut self, env: &Env<'_>) {
        let (start, end) = (self.page.start, self.page.end);
        let to = if end.section == start.section {
            end.offset
        } else {
            u32::MAX
        };
        self.links = self
            .document
            .links(env.storage, start.section, start.offset, to);
    }

    /// The page's links that can be followed (to the web only if a
    /// browser is there), in reading order: indexes into `links`.
    fn followable(&self, env: &Env<'_>) -> Vec<usize> {
        let web = env.has_slot(&slots::BROWSER);
        let mut order: Vec<usize> = (0..self.links.len())
            .filter(|&i| web || !quireink_sdk::document::is_external(&self.links[i].href))
            .collect();
        order.sort_by_key(|&i| self.links[i].start);
        order
    }

    /// The text at the target of link `href`, as a note shows it: the
    /// block there and the ones after it, up to a heading or a few hundred
    /// characters.
    fn note_text(&self, env: &Env<'_>, href: &str) -> Option<String> {
        let mut at = self.document.link_target(env.storage, href)?;
        let mut text = String::new();
        for _ in 0..4 {
            let Ok(Some(block)) = self.document.block_at(env.storage, at) else {
                break;
            };
            if matches!(block.kind, quireink_sdk::document::BlockKind::Heading(_))
                && !text.is_empty()
            {
                break;
            }
            let part = block.text.trim();
            if !part.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(part);
            }
            // A note that is only its number goes on in the next block.
            if text.chars().count() > 300 || text.chars().count() > 12 && block.end > block.start {
                break;
            }
            at = Locator::new(at.section, block.end);
        }
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!text.is_empty()).then_some(text)
    }

    /// The link `word` is part of, if any.
    fn link_of(&self, word: &WordBox) -> Option<&Link> {
        if word.start.section != self.page.start.section {
            return None;
        }
        self.links
            .iter()
            .find(|l| word.end.offset > l.start && word.start.offset < l.end)
    }

    /// Shows the page that starts at `start`, far from here (a link, the
    /// contents, a search hit, a page number): *Back* returns here.
    fn jump(&mut self, cx: &mut Cx<'_, '_>, start: Locator) -> bool {
        let from = self.page.start;
        if !self.show(cx, start, true) {
            return false;
        }
        if self.returns.len() == RETURNS {
            self.returns.remove(0);
        }
        self.returns.push(from);
        self.landed = Some(self.page.start);
        true
    }

    /// Back to where the last jump was made from.
    fn go_back(&mut self, cx: &mut Cx<'_, '_>) {
        if let Some(from) = self.returns.pop() {
            self.landed = None;
            self.show(cx, from, false);
        }
    }

    /// Where *Back* is offered on the page a link led to: a small button
    /// in the top margin, on the left.
    /// On web pages it is always there: back to the page before.
    fn back_area(&self, env: &Env<'_>) -> Option<Rect> {
        let landed = !self.returns.is_empty() && self.landed == Some(self.page.start);
        if !landed && !self.web() {
            return None;
        }
        let style = UiStyle::new(env);
        let font = style.font(env)?;
        let label = env.i18n.tr("reader.link-back");
        let pad = style.gap as f32;
        let width = font.measure(style.small, label) + 2.0 * pad;
        // Within the top margin where it is wide enough, so it covers no
        // text.
        let line = font.line_metrics(style.small).line_height;
        let height = (line + pad).min(self.typography.y - 4.0).max(line);
        let y = ((self.typography.y - height) / 2.0).max(0.0);
        Some(Rect::new(
            self.typography.x as u16,
            y as u16,
            width as u16,
            height as u16,
        ))
    }

    fn draw_back(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let Some(area) = self.back_area(env) else {
            return;
        };
        let style = UiStyle::new(env);
        let Some(font) = style.font(env) else {
            return;
        };
        let (x, y, w, h) = (
            i32::from(area.x),
            i32::from(area.y),
            u32::from(area.width),
            u32::from(area.height),
        );
        canvas.fill_rect(x, y, w, h, Color::WHITE);
        canvas.stroke_rect(x, y, w, h, 1, Color::BLACK);
        let label = env.i18n.tr("reader.link-back");
        ui::draw_label(
            canvas,
            &font,
            style.small,
            f32::from(area.x) + style.gap as f32,
            area,
            label,
            Color::BLACK,
        );
    }

    /// Links underlined thinly, so they can be found without standing out.
    /// Links to the web only if a browser is there to open them.
    fn draw_links(&self, canvas: &mut Canvas<'_>, env: &Env<'_>, words: &[WordBox]) {
        let web = env.has_slot(&slots::BROWSER);
        let followed = |i: usize| {
            self.link_of(&words[i])
                .is_some_and(|link| web || !quireink_sdk::document::is_external(&link.href))
        };
        for b in selection::line_boxes(words, followed) {
            canvas.fill_rect(
                i32::from(b.x),
                b.bottom() as i32 - 2,
                u32::from(b.width),
                1,
                Color::gray(90),
            );
        }
    }

    /// Whether the page has a bookmark.
    fn bookmarked(&self) -> bool {
        let (start, end) = (self.page.start, self.page.end);
        self.marks
            .iter()
            .any(|m| m.kind == Kind::Bookmark && m.start >= start && m.start < end)
    }

    /// Where the bookmark ribbon is: in the top margin, on the right.
    /// The start of the page that contains `at`, as the page index has it
    /// (so pages look the same as when paging there); `at` itself where
    /// the index does not reach yet.
    /// Where the book is, for the skimming overlay: the page, the chapter
    /// and how far, in thousandths.
    fn skim_params(&self, env: &Env<'_>) -> Params {
        let chapter = self
            .chapter
            .as_ref()
            .map(|(_, _, title)| title.clone())
            .unwrap_or_default();
        Params::new()
            .with(params::PATH, self.path.clone())
            .with(params::PAGE, self.position_label(env))
            .with(params::TEXT, chapter)
            .with(
                crate::skim::AT,
                format!("{}", (self.percent() * 10.0) as u32),
            )
    }

    /// Where `to` leads: `page:±n` pages on (or as many tenths of a
    /// percent while pages are counted), `permille:n` that far into the
    /// book, `chapter:±1` the chapter before or after.
    fn skim_target(&self, env: &Env<'_>, to: &str) -> Option<Locator> {
        let (kind, value) = to.split_once(':')?;
        let value: i64 = value.trim_start_matches('+').parse().ok()?;
        let pages = self
            .index
            .as_ref()
            .filter(|i| i.is_complete() && i.pages() > 0);
        match kind {
            "page" => {
                if let Some(index) = pages
                    && let Some((page, _)) = index.locate(env.storage, self.page.start)
                {
                    let target = (i64::from(page) + value).clamp(0, i64::from(index.pages()) - 1);
                    return index.entry(env.storage, target as u32);
                }
                let at = (self.percent() as f64 * 10.0 + value as f64).clamp(0.0, 999.0);
                Some(self.locator_at(at / 1000.0))
            }
            "permille" => {
                let fraction = value.clamp(0, 999) as f64 / 1000.0;
                if let Some(index) = pages {
                    let page = (fraction * f64::from(index.pages())) as u32;
                    return index.entry(env.storage, page.min(index.pages() - 1));
                }
                Some(self.locator_at(fraction))
            }
            "chapter" => {
                // Where the top-level entries begin, exactly (their anchors).
                let storage = env.storage;
                let mut starts: Vec<Locator> = (0..self.document.toc_len(storage).min(2000))
                    .filter(|&i| {
                        self.document
                            .toc_entry(storage, i)
                            .is_some_and(|e| e.level == 0)
                    })
                    .filter_map(|i| self.document.toc_target(storage, i))
                    .collect();
                starts.sort();
                starts.dedup();
                let here = self.page.start;
                let current = starts.iter().rposition(|&s| s <= here);
                let target = match (value < 0, current) {
                    // Back: to the start of this chapter, or the one before.
                    (true, Some(i)) if starts[i] < here => Some(i),
                    (true, Some(i)) => i.checked_sub(1),
                    (true, None) => None,
                    (false, Some(i)) => Some(i + 1),
                    (false, None) => Some(0),
                };
                target.and_then(|i| starts.get(i).copied())
            }
            _ => None,
        }
    }

    /// The position `fraction` of the way through the book, by the
    /// sections' lengths.
    fn locator_at(&self, fraction: f64) -> Locator {
        let sections = self.document.sections();
        let total: u64 = (0..sections)
            .map(|s| u64::from(self.document.section_len(s)))
            .sum();
        let mut left = (fraction.clamp(0.0, 1.0) * total as f64) as u64;
        for section in 0..sections {
            let len = u64::from(self.document.section_len(section));
            if left < len {
                return Locator::new(section, left as u32);
            }
            left -= len;
        }
        Locator::new(sections.saturating_sub(1), 0)
    }

    fn page_start(&self, env: &Env<'_>, at: Locator) -> Locator {
        let Some(index) = &self.index else {
            return at;
        };
        match index.locate(env.storage, at) {
            Some((page, _)) if index.is_complete() || page + 1 < index.pages() => {
                index.entry(env.storage, page).unwrap_or(at)
            }
            _ => at,
        }
    }

    /// Where a tap sets or removes a bookmark: the top right corner.
    fn corner_area(&self, env: &Env<'_>) -> Rect {
        let display = env.device.display;
        let size = (env.metrics.mm(12) as u16).min(display.width / 4);
        Rect::new(display.width - size, 0, size, size)
    }

    fn ribbon_area(&self, env: &Env<'_>) -> Rect {
        let t = &self.typography;
        let width = env.metrics.mm(3) as u16;
        let height = (t.y * 0.8) as u16;
        let x = (t.x + t.width) as u16 - width;
        Rect::new(x, 0, width, height.max(width))
    }

    /// A ribbon with a notch, like a bookmark in a book.
    fn draw_ribbon(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let r = self.ribbon_area(env);
        let notch = u32::from(r.width) / 2;
        let body = u32::from(r.height).saturating_sub(notch);
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            body,
            Color::BLACK,
        );
        // Both sides of the notch narrow towards the bottom.
        for row in 0..notch {
            let side = notch - row;
            let y = i32::from(r.y) + (body + row) as i32;
            canvas.fill_rect(i32::from(r.x), y, side, 1, Color::BLACK);
            canvas.fill_rect(
                i32::from(r.x) + (u32::from(r.width) - side) as i32,
                y,
                side,
                1,
                Color::BLACK,
            );
        }
    }

    /// Highlights behind the text: light gray, one shape per line.
    fn draw_highlights(&self, canvas: &mut Canvas<'_>, env: &Env<'_>, words: &[WordBox]) {
        let highlights: Vec<&Mark> = self
            .marks
            .iter()
            .filter(|m| m.kind == Kind::Highlight && m.overlaps(self.page.start, self.page.end))
            .collect();
        if highlights.is_empty() {
            return;
        }
        let boxes = selection::line_boxes(words, |i| {
            highlights
                .iter()
                .any(|m| m.overlaps(words[i].start, words[i].end))
        });
        let underline = env.settings.text(crate::settings::HIGHLIGHT_STYLE.as_str())
            == crate::settings::HIGHLIGHT_UNDERLINE;
        let line = env.metrics.tenth_mm(4).max(2);
        for b in boxes {
            if underline {
                canvas.fill_rect(
                    i32::from(b.x),
                    b.bottom() as i32 - line as i32,
                    u32::from(b.width),
                    line,
                    Color::BLACK,
                );
            } else {
                canvas.fill_rect(
                    i32::from(b.x) - 2,
                    i32::from(b.y),
                    u32::from(b.width) + 4,
                    u32::from(b.height),
                    Color::gray(205),
                );
            }
        }
        // A note's sign in the right margin, beside its first line.
        let style = UiStyle::new(env);
        let size = style.small;
        let x = self.typography.x + self.typography.width + (style.gap as f32) / 2.0;
        for mark in highlights.iter().filter(|m| m.noted) {
            if let Some(first) = words.iter().find(|w| mark.overlaps(w.start, w.end)) {
                ui::draw_icon(canvas, env, "note", size, x, first.area, Color::BLACK);
            }
        }
    }

    /// What status items get to know about the book.
    fn status_context(&self, env: &Env<'_>) -> Params {
        let mut context = self
            .event_params(env)
            .with(status_context::PAGE, self.position_label(env));
        if let Some((_, _, chapter)) = &self.chapter
            && !chapter.is_empty()
        {
            context.set(status_context::CHAPTER, chapter.clone());
        }
        if !self.chapter_marks.is_empty() {
            context.set(status_context::CHAPTERS, self.chapter_marks.clone());
        }
        if let Some(icon) = self.hands_free {
            context.set(status::ALOUD_CONTEXT, icon);
        }
        if let Some(page) = self.document.page_label(env.storage, self.page.start) {
            context.set(status::PRINTED_CONTEXT, page);
        }
        context
    }

    /// Where the top-level chapters begin, as thousandths of the book.
    fn chapter_marks(&self, env: &Env<'_>) -> String {
        let (document, storage) = (&self.document, env.storage);
        let mut marks: Vec<u32> = Vec::new();
        for index in 0..document.toc_len(storage).min(500) {
            if let Some(entry) = document.toc_entry(storage, index).filter(|e| e.level == 0) {
                marks.push((document.progress(entry.at) * 1000.0) as u32);
            }
        }
        marks.dedup();
        let marks: Vec<String> = marks.iter().map(ToString::to_string).collect();
        marks.join(",")
    }

    /// The status line at the top (or bottom) that `point` is on: its
    /// place reaches to the edge of the screen.
    fn status_hit(&self, env: &Env<'_>, point: Point) -> Option<bool> {
        let height = env.device.display.height;
        self.status_areas().find_map(|(area, top)| {
            let zone = if top {
                Rect::new(0, 0, env.device.display.width, area.y + area.height)
            } else {
                Rect::new(0, area.y, env.device.display.width, height - area.y)
            };
            zone.contains(point).then_some(top)
        })
    }

    /// The status lines' areas.
    fn status_areas(&self) -> impl Iterator<Item = (Rect, bool)> {
        let t = &self.typography;
        t.top_status
            .map(|r| (r, true))
            .into_iter()
            .chain(t.status.map(|r| (r, false)))
    }

    fn draw_status(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let context = self.status_context(env);
        for (area, top) in self.status_areas() {
            canvas.fill_rect(
                i32::from(area.x),
                i32::from(area.y),
                u32::from(area.width),
                u32::from(area.height),
                Color::WHITE,
            );
            let id = status::current(env, top, &self.status_lines[usize::from(!top)]);
            let frame = quireink_sdk::StatusFrame {
                area,
                top,
                size: status::size(env),
                color: status::COLOR,
                context: &context,
            };
            env.status_items.draw_line(id, canvas, env, &frame);
        }
    }

    /// Finds the chapter of the page when a status line shows it; the
    /// table of contents is read only when the page leaves the chapter.
    fn update_chapter(&mut self, env: &Env<'_>) {
        let at = self.page.start;
        if let Some((from, to, _)) = &self.chapter
            && *from <= at
            && to.is_none_or(|to| at < to)
        {
            return;
        }
        self.chapter = None;
        if !status::uses(env, &status::items::CHAPTER) {
            return;
        }
        let (document, storage) = (&self.document, env.storage);
        let mut chapter = (Locator::START, None, String::new());
        for index in 0..document.toc_len(storage) {
            let Some(entry) = document.toc_entry(storage, index) else {
                continue;
            };
            // Entries in the page's section need their exact position.
            let start = if entry.at.section == at.section {
                document.toc_target(storage, index).unwrap_or(entry.at)
            } else {
                entry.at
            };
            if start <= at {
                chapter = (start, None, entry.title);
            } else {
                chapter.1 = Some(start);
                break;
            }
        }
        self.chapter = Some(chapter);
    }
}

/// The text of `link` on the page: the parts of its words inside it (a
/// note mark is often part of a word, as in `source¹`).
fn link_label(book: &Book, words: &[WordBox], link: &Link) -> String {
    let mut label = String::new();
    for word in words.iter().filter(|w| {
        w.start.section == book.page.start.section
            && w.end.offset > link.start
            && w.start.offset < link.end
    }) {
        let from = link.start.saturating_sub(word.start.offset) as usize;
        let to = (link.end.min(word.end.offset) - word.start.offset) as usize;
        let part = word.text.get(from..to).unwrap_or(&word.text);
        if !label.is_empty() {
            label.push(' ');
        }
        label.push_str(part);
    }
    label
}

/// The link being chosen, framed like selected text, and a bar at the
/// bottom saying where it leads and what the buttons do.
fn draw_linking(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    book: &Book,
    words: &[WordBox],
    linking: Linking,
) {
    let link = linking
        .focus
        .and_then(|i| book.followable(env).get(i).copied())
        .map(|i| &book.links[i]);
    if let Some(link) = link {
        let line = env.metrics.tenth_mm(4).max(2);
        let inside = |i: usize| {
            words[i].start.section == book.page.start.section
                && words[i].end.offset > link.start
                && words[i].start.offset < link.end
        };
        for b in selection::line_boxes(words, inside) {
            canvas.stroke_rect(
                i32::from(b.x) - 4,
                i32::from(b.y) - 2,
                u32::from(b.width) + 8,
                u32::from(b.height) + 4,
                line,
                Color::BLACK,
            );
        }
    }
    let style = UiStyle::new(env);
    let bar = selection::bar_area(env, &style);
    ui::fill(canvas, bar, Color::WHITE);
    canvas.fill_rect(
        i32::from(bar.x),
        i32::from(bar.y),
        u32::from(bar.width),
        style.line.max(2),
        Color::BLACK,
    );
    let Some(font) = style.font(env) else {
        return;
    };
    let text = match link {
        Some(link) => {
            let place = if quireink_sdk::document::is_external(&link.href) {
                quireink_sdk::web::host(&link.href).to_string()
            } else {
                env.i18n.tr("reader.link.in-book").to_string()
            };
            format!("{place} · {}", env.i18n.tr("reader.link.hint"))
        }
        None => env.i18n.tr("reader.link.none").to_string(),
    };
    let margin = style.margin as f32;
    let text = ui::fit(
        &font,
        style.small,
        &text,
        f32::from(bar.width) - 2.0 * margin,
    );
    ui::draw_label(
        canvas,
        &font,
        style.small,
        f32::from(bar.x) + margin,
        bar,
        &text,
        Color::BLACK,
    );
}

impl Screen for ReaderScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let book = match &self.book {
            Ok(book) => book,
            Err(message) => return draw_message(canvas, env, message),
        };
        // Word boxes are measured only when something is drawn on words.
        let has_highlights = book.marks.iter().any(|m| m.kind == Kind::Highlight);
        let words = if has_highlights
            || self.selection.is_some()
            || self.linking.is_some()
            || book.found.is_some()
            || self.aloud.is_some()
            || !book.links.is_empty()
        {
            book.words(env)
        } else {
            Vec::new()
        };
        book.draw_highlights(canvas, env, &words);
        book.draw_page(canvas, env);
        book.draw_links(canvas, env, &words);
        book.draw_back(canvas, env);
        book.draw_status(canvas, env);
        if book.bookmarked() {
            book.draw_ribbon(canvas, env);
        }
        // The sentence read aloud, underlined like a search hit.
        if let Some(aloud) = self.aloud
            && aloud.now.page == book.page.start
            && env.settings.bool(crate::settings::ALOUD_HIGHLIGHT.as_str())
        {
            let line = env.metrics.tenth_mm(5).max(2);
            let sentence = aloud.now.from..aloud.now.to;
            for b in selection::line_boxes(&words, |i| sentence.contains(&i)) {
                canvas.fill_rect(
                    i32::from(b.x),
                    b.bottom() as i32 + 1,
                    u32::from(b.width),
                    line,
                    Color::BLACK,
                );
            }
        }
        if let Some((start, end)) = book.found {
            // Underlined, so it stays visible under a highlight.
            let line = env.metrics.tenth_mm(5).max(2);
            let found = |i: usize| words[i].end > start && words[i].start < end;
            for b in selection::line_boxes(&words, found) {
                canvas.fill_rect(
                    i32::from(b.x),
                    b.bottom() as i32 + 1,
                    u32::from(b.width),
                    line,
                    Color::BLACK,
                );
            }
        }
        // A bar at the bottom hides the status line under it entirely.
        let bar = self.linking.is_some() || self.selection.is_some_and(|s| s.mode == Mode::Bar);
        if let Some(status) = book.typography.status.filter(|_| bar) {
            ui::fill(canvas, status, Color::WHITE);
        }
        if let Some(linking) = self.linking {
            draw_linking(canvas, env, book, &words, linking);
        }
        let Some(selection) = self.selection else {
            return;
        };
        let (lo, hi) = selection.range();
        let line = env.metrics.tenth_mm(4).max(2);
        for b in selection::line_boxes(&words, |i| (lo..=hi).contains(&i)) {
            canvas.stroke_rect(
                i32::from(b.x) - 4,
                i32::from(b.y) - 2,
                u32::from(b.width) + 8,
                u32::from(b.height) + 4,
                line,
                Color::BLACK,
            );
        }
        if selection.mode == Mode::Bar {
            let items = self.items(env, &words);
            selection::draw_bar(canvas, env, &items, selection.focus);
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        // Done skimming: *Back* returns to where it began.
        if let (Some(from), Ok(book)) = (self.skim_from.take(), &mut self.book)
            && book.page.start != from
        {
            if book.returns.len() == RETURNS {
                book.returns.remove(0);
            }
            book.returns.push(from);
            book.landed = Some(book.page.start);
        }
        // *Back* is drawn anew: screens over it may have hidden it.
        if let Ok(book) = &self.book
            && let Some(area) = book.back_area(cx.env())
        {
            cx.request_redraw_area(area);
        }
        if let Ok(book) = &self.book {
            // The book's own settings ("Only this book") while it is open.
            cx.command(Command::SetScope(Some(format!(
                "{}/settings.ini",
                book.dir
            ))));
            if !book.web() {
                let params = book.event_params(cx.env());
                cx.command(Command::Emit(events::BOOK_OPENED, params));
            }
            let now = cx.env().now;
            cx.wake_at(now);
        }
        // Time passed while another screen was open.
        self.update_status(cx);
    }

    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Ok(book) = &mut self.book else {
            return;
        };
        if book.index_step(&env) {
            cx.wake_at(env.now.add_millis(STEP_DELAY_MS));
        }
        self.auto_turn_due(cx);
        // Page numbers once counted, the time, the battery, …
        self.update_status(cx);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        // Any button pauses reading aloud (also "Read aloud"), and does
        // nothing else.
        if self.aloud.is_some() {
            self.pause_aloud(cx);
            return Handled::Yes;
        }
        if *action == crate::actions::READ_ALOUD {
            self.start_aloud(cx);
            return Handled::Yes;
        }
        if *action == crate::actions::AUTO_TURN {
            self.toggle_auto_turn(cx);
            return Handled::Yes;
        }
        if *action == crate::actions::SKIM_BOOK {
            self.open_skim(cx);
            return Handled::Yes;
        }
        // While pages turn by themselves, turning one starts the count
        // anew; the menu (a tap in the middle) stops it instead.
        if self.auto_turn.is_some() {
            let env = *cx.env();
            if *action == actions::MENU || *action == actions::CONFIRM {
                self.toggle_auto_turn(cx);
                return Handled::Yes;
            }
            if *action == actions::NEXT_PAGE || *action == actions::PREV_PAGE {
                let seconds = self.auto_turn_seconds(&env);
                self.auto_turn = Some(env.now.add_millis(seconds * 1000));
                if let Some(at) = self.auto_turn {
                    cx.wake_at(at);
                }
            }
        }
        if let Some(selection) = self.selection {
            return self.on_selection(action, selection, cx);
        }
        if let Some(linking) = self.linking {
            return self.on_linking(action, linking, cx);
        }
        if *action == crate::actions::LINKS {
            let env = *cx.env();
            if let Ok(book) = &self.book {
                let focus = (!book.followable(&env).is_empty()).then_some(0);
                self.set_linking(Some(Linking { focus }), cx);
            }
            return Handled::Yes;
        }
        if *action == crate::actions::SELECT {
            // Choose text with the buttons, starting at the first word.
            let env = *cx.env();
            if let Ok(book) = &self.book {
                let words = book.words(&env);
                let first = words.iter().position(|w| !w.lookup_text().is_empty());
                self.set_selection(first.map(Selection::choosing), cx);
            }
            return Handled::Yes;
        }
        if *action == crate::actions::TOGGLE_BOOKMARK {
            self.toggle_bookmark(cx);
            return Handled::Yes;
        }
        if *action == crate::actions::NEXT_STATUS_LINE
            || *action == crate::actions::NEXT_TOP_STATUS_LINE
        {
            let env = *cx.env();
            let top =
                *action == crate::actions::NEXT_TOP_STATUS_LINE || !status::shown(&env, false);
            self.next_status_line(top, cx);
            return Handled::Yes;
        }
        if *action == crate::actions::SEARCH || *action == crate::actions::GO_TO_PAGE {
            self.ask(action, cx);
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
        } else if *action == crate::actions::BOOK_INFO {
            cx.command(Command::OpenSlot(
                slots::BOOK_INFO,
                Params::new().with(params::PATH, book.path.clone()),
            ));
            Handled::Yes
        } else if *action == crate::actions::TOC {
            cx.command(Command::OpenScreen(
                crate::TOC_SCREEN,
                crate::toc::locator_params(&book.path, book.page.start),
            ));
            Handled::Yes
        } else if *action == crate::actions::ANNOTATIONS {
            cx.command(Command::OpenScreen(
                crate::ANNOTATIONS_SCREEN,
                Params::new().with(params::PATH, book.path.clone()),
            ));
            Handled::Yes
        } else if *action == actions::BACK && !book.returns.is_empty() {
            book.go_back(cx);
            Handled::Yes
        } else if *action == actions::MENU || *action == actions::CONFIRM {
            // As a bar at the top, with the book and where one is in it.
            let env = *cx.env();
            let chapter = book
                .chapter
                .as_ref()
                .map(|(_, _, title)| title.clone())
                .unwrap_or_default();
            let event = book.event_params(&env);
            let page = match (event.get(params::PAGE), event.get(params::PAGES)) {
                (Some(page), Some(pages)) => format!("{page} / {pages}"),
                _ => format!("{} %", event.get(params::PROGRESS).unwrap_or("0")),
            };
            // Pictures of pages: no text to search, hear or set.
            let mut hide: Vec<Id> = Vec::new();
            if book.document.whole_pages() {
                hide.extend([
                    crate::actions::SEARCH,
                    crate::actions::READ_ALOUD,
                    crate::actions::QUICK_SETTINGS,
                ]);
                if book.document.toc_len(env.storage) == 0 {
                    hide.push(crate::actions::TOC);
                }
            }
            let hide: Vec<&str> = hide.iter().map(Id::as_str).collect();
            cx.command(Command::OpenSlot(
                slots::MENU,
                Params::new()
                    .with(params::MENU, crate::MENU.as_str())
                    .with(params::HIDE, hide.join(","))
                    .with(params::MODE, "bar")
                    .with(params::TITLE, book.title.clone())
                    .with(params::TEXT, chapter)
                    .with(params::PAGE, page)
                    .with(params::ACTION, crate::actions::BOOK_INFO.as_str()),
            ));
            Handled::Yes
        } else {
            Handled::No
        }
    }

    /// A tap on a link follows it, a long press selects the word under
    /// the finger; while text is selected, taps extend the selection or
    /// choose an item of the bar.
    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        // As does any tap.
        if self.aloud.is_some() {
            self.pause_aloud(cx);
            return Handled::Yes;
        }
        if self.linking.is_some() {
            self.set_linking(None, cx);
            return Handled::Yes;
        }
        let env = *cx.env();
        // A tap on a status line shows the next one (not where the
        // bookmark corner or the way back are).
        if let (GestureKind::Tap, None, Ok(book)) = (gesture.kind, self.selection, &self.book) {
            let p = gesture.start;
            let corner = env.settings.bool(crate::settings::CORNER_BOOKMARK.as_str())
                && book.corner_area(&env).contains(p);
            let back = book.back_area(&env).is_some_and(|a| a.contains(p));
            if !corner
                && !back
                && let Some(top) = book.status_hit(&env, p)
            {
                self.next_status_line(top, cx);
                return Handled::Yes;
            }
        }
        let Ok(book) = &self.book else {
            return Handled::No;
        };
        let words = book.words(&env);
        let at = |point: Point| {
            words.iter().position(|word| {
                let area = word.area;
                // A little slack between words and lines.
                let slack = 4;
                point.x >= i32::from(area.x) - slack
                    && point.x < area.right() as i32 + slack
                    && point.y >= i32::from(area.y)
                    && point.y < area.bottom() as i32
            })
        };
        let link = at(gesture.start)
            .and_then(|i| book.link_of(&words[i]))
            .map(|link| (link.href.clone(), link_label(book, &words, link)));
        match (gesture.kind, self.selection) {
            (GestureKind::Tap, None)
                if env.settings.bool(crate::settings::CORNER_BOOKMARK.as_str())
                    && book.corner_area(&env).contains(gesture.start) =>
            {
                self.toggle_bookmark(cx);
                Handled::Yes
            }
            (GestureKind::Tap, None)
                if book
                    .back_area(&env)
                    .is_some_and(|area| area.contains(gesture.start)) =>
            {
                if let Ok(book) = &mut self.book {
                    if book.returns.is_empty() {
                        // Out of the web page, to the one before.
                        cx.command(Command::Back);
                    } else {
                        book.go_back(cx);
                    }
                }
                Handled::Yes
            }
            (GestureKind::Tap, None) if link.is_some() => {
                let (href, label) = link.unwrap_or_default();
                self.open_link(&href, &label, cx)
            }
            (GestureKind::LongPress, _) => match at(gesture.start) {
                Some(word) => {
                    self.set_selection(Some(Selection::touched(word)), cx);
                    Handled::Yes
                }
                None => Handled::No,
            },
            (GestureKind::Tap, Some(selection)) => {
                let items = self.items(&env, &words);
                if let Some(item) = selection::item_at(&env, &items, gesture.start) {
                    self.activate(&item.id, cx);
                } else if let Some(word) = at(gesture.start) {
                    let to = Selection {
                        cursor: word,
                        mode: Mode::Bar,
                        ..selection
                    };
                    self.set_selection(Some(to), cx);
                } else {
                    self.set_selection(None, cx);
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
        let this_book = params.get(params::PATH) == Some(book.path.as_str());
        if *event == crate::SPOKEN && this_book {
            self.spoken(params, cx);
            return;
        }
        if *event == crate::FOLLOW && this_book {
            let href = params
                .get(crate::footnote::HREF)
                .unwrap_or_default()
                .to_string();
            self.follow(&href, cx);
            return;
        }
        if *event == crate::GOTO
            && this_book
            && let Some(at) = crate::toc::locator_of(params)
        {
            // Another page: no text stays selected.
            self.selection = None;
            let env = *cx.env();
            let start = book.page_start(&env, at);
            book.jump(cx, start);
            let end = params
                .get(crate::END_SECTION)
                .zip(params.get(crate::END_OFFSET))
                .and_then(|(s, o)| Some(Locator::new(s.parse().ok()?, o.parse().ok()?)));
            if let Some(end) = end {
                book.found = Some((at, end));
                cx.request_redraw();
            }
            return;
        }
        if *event == crate::SKIM && this_book {
            let to = params.get(crate::skim::TO).unwrap_or_default().to_string();
            self.skim(&to, cx);
            return;
        }
        if *event == crate::NOTE_ENTERED
            && this_book
            && let Some(at) = crate::toc::locator_of(params)
        {
            let env = *cx.env();
            let note = params.get(params::TEXT).unwrap_or_default();
            annotations::set_note(env.storage, &book.dir, at, note);
            book.marks = annotations::marks(env.storage, &book.dir);
            cx.request_redraw();
            return;
        }
        if *event == crate::SEARCH_ENTERED && this_book {
            let text = params.get(params::TEXT).unwrap_or_default().trim();
            if !text.is_empty() {
                self.last_search = text.to_string();
                cx.command(Command::OpenScreen(
                    crate::SEARCH_SCREEN,
                    Params::new()
                        .with(params::PATH, book.path.clone())
                        .with(params::TEXT, text.to_string()),
                ));
            }
            return;
        }
        if *event == crate::PAGE_ENTERED && this_book {
            let env = *cx.env();
            let number = params
                .get(params::TEXT)
                .and_then(|t| t.trim().parse::<u32>().ok());
            if let (Some(number), Some(index)) = (number, &book.index)
                && index.pages() > 0
            {
                let page = number.clamp(1, index.pages()) - 1;
                if let Some(start) = index.entry(env.storage, page) {
                    self.selection = None;
                    book.jump(cx, start);
                }
            }
            return;
        }
        let font_changed = *event == events::SETTING_CHANGED
            && params.get(params::ID)
                == Some(quireink_sdk::builtin::settings::READING_FONT.as_str());
        if *event == events::STATUS_CHANGED
            || *event == events::SETTING_CHANGED
                && !font_changed
                && params
                    .get(params::ID)
                    .is_some_and(|id| id.starts_with("core."))
        {
            // The battery, the clock format, the time zone, …
            self.status_shown.clear();
            self.update_status(cx);
            return;
        }
        let Ok(book) = &mut self.book else {
            return;
        };
        let id = params.get(params::ID).unwrap_or_default();
        // The book's fonts are loaded with it.
        let reopen = [
            crate::settings::PUBLISHER_STYLES,
            quireink_sdk::builtin::settings::READING_FONT,
        ];
        if *event == events::SETTING_CHANGED && reopen.iter().any(|r| r.as_str() == id) {
            // Another conversion of the book: open it again where it was.
            let env = *cx.env();
            let path = book.path.clone();
            self.selection = None;
            self.book = Book::open(&env, &Params::new().with(params::PATH, path));
            cx.request_redraw();
            cx.wake_at(env.now);
            return;
        }
        let filters = env_filters_changed(cx.env(), book);
        let affects_layout = font_changed
            || filters
            || id.starts_with("reader.") && id != crate::settings::OPEN_LAST.as_str();
        if *event == events::SETTING_CHANGED && affects_layout {
            self.selection = None;
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

/// Whether the text filters do something else now than when the pages
/// were laid out.
fn env_filters_changed(env: &Env<'_>, book: &Book) -> bool {
    book.typography.filters != env.text_filters.signature(env)
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

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// FNV-1a, for short stable ids of paths and layouts.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}
