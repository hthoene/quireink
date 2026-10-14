use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;
use core::ops::Range;

use quireink_graphics::Canvas;
use quireink_platform::{Color, Storage};
use quireink_text::sfnt::FontInfo;
use quireink_text::{Font, GlyphCache, LineMetrics};

use crate::builtin::fonts as roles;
use crate::collections::VecMap;
use crate::id::Id;
use crate::plugin::{Priority, RegisterError};

/// A font contribution.
#[derive(Debug, Clone)]
pub struct FontSpec {
    /// `<plugin>.<name>`, e.g. `fonts-default.literata`.
    pub id: Id,
    pub font: Font,
    /// The family it belongs to, e.g. `Literata`: families are what users
    /// choose (with their bold and italic faces). `None` keeps a font out
    /// of the choice.
    pub family: Option<Cow<'static, str>>,
    pub bold: bool,
    pub italic: bool,
}

impl FontSpec {
    /// A font outside any family.
    pub fn new(id: Id, font: Font) -> Self {
        Self {
            id,
            font,
            family: None,
            bold: false,
            italic: false,
        }
    }

    /// Puts the font into `family` as its regular, bold, italic or bold
    /// italic face.
    pub fn family(
        mut self,
        family: impl Into<Cow<'static, str>>,
        bold: bool,
        italic: bool,
    ) -> Self {
        self.family = Some(family.into());
        self.bold = bold;
        self.italic = italic;
        self
    }
}

/// One registered font.
#[derive(Debug)]
struct Entry {
    id: Id,
    /// `None` for a font file not loaded (yet).
    font: Option<Font>,
    family: Option<Cow<'static, str>>,
    bold: bool,
    italic: bool,
    /// 400 regular, 700 bold; picks among faces of the same style.
    weight: u16,
    /// A font file on the storage, loaded only while its family is chosen.
    file: Option<String>,
}

/// Largest font file loaded.
const MAX_FONT_FILE: u64 = 48 * 1024 * 1024;

/// All registered fonts, the role bindings and the glyph cache.
///
/// Fonts are bound to *roles* such as
/// [`builtin::fonts::UI`](crate::builtin::fonts::UI) — like screens to slots.
/// Plugins propose fonts for roles; a chosen reading family
/// ([`Fonts::set_family`]) fills the reading roles, and the user's
/// `[fonts]` configuration overrides everything.
pub struct Fonts {
    entries: Vec<Entry>,
    candidates: Vec<(Id, Id, Priority)>,
    user: VecMap<Id, Id>,
    /// The family chosen for reading.
    family: Option<String>,
    cache: RefCell<GlyphCache>,
}

impl fmt::Debug for Fonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fonts")
            .field(
                "fonts",
                &self.entries.iter().map(|e| &e.id).collect::<Vec<_>>(),
            )
            .field("family", &self.family)
            .finish_non_exhaustive()
    }
}

impl Fonts {
    /// `cache_bytes` bounds the memory for glyphs rasterized at runtime.
    pub fn new(cache_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            candidates: Vec::new(),
            user: VecMap::new(),
            family: None,
            cache: RefCell::new(GlyphCache::new(cache_bytes)),
        }
    }

    fn check_new(&self, id: &Id) -> Result<(), RegisterError> {
        if self.entries.iter().any(|e| e.id == *id) {
            return Err(RegisterError::Duplicate { id: id.clone() });
        }
        Ok(())
    }

    pub fn add(&mut self, spec: FontSpec) -> Result<(), RegisterError> {
        self.check_new(&spec.id)?;
        self.entries.push(Entry {
            id: spec.id,
            font: Some(spec.font),
            family: spec.family,
            weight: if spec.bold { 700 } else { 400 },
            bold: spec.bold,
            italic: spec.italic,
            file: None,
        });
        Ok(())
    }

    /// Adds a font file on the storage, described by its header (see
    /// [`sfnt::read_info`](quireink_text::sfnt::read_info)); it is loaded
    /// only when its family is chosen.
    pub fn add_file(&mut self, id: Id, path: String, info: FontInfo) -> Result<(), RegisterError> {
        self.check_new(&id)?;
        self.entries.push(Entry {
            id,
            font: None,
            family: Some(Cow::Owned(info.family)),
            bold: info.bold,
            italic: info.italic,
            weight: info.weight,
            file: Some(path),
        });
        Ok(())
    }

    /// Proposes `font` for `role`; the highest priority wins.
    pub fn propose(&mut self, role: Id, font: Id, priority: Priority) {
        self.candidates.push((role, font, priority));
    }

    /// Releases unused capacity once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
        self.candidates.shrink_to_fit();
        self.user.shrink_to_fit();
    }

    /// The user's explicit choice for a role.
    pub fn bind_user(&mut self, role: Id, font: Id) {
        self.user.insert(role, font);
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id.as_str() == id)
    }

    /// Ids of all fonts, loaded or not.
    pub fn ids(&self) -> impl Iterator<Item = &Id> {
        self.entries.iter().map(|e| &e.id)
    }

    /// The families to choose from, in registration order.
    pub fn families(&self) -> Vec<&str> {
        let mut families: Vec<&str> = Vec::new();
        for family in self.entries.iter().filter_map(|e| e.family.as_deref()) {
            if !families.contains(&family) {
                families.push(family);
            }
        }
        families
    }

    /// The family chosen for reading, if any.
    pub fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    /// Chooses the family for reading (`None`: the plugins' proposals).
    /// Call [`Fonts::load`] afterwards for font files.
    pub fn set_family(&mut self, family: Option<&str>) {
        self.family = family.map(String::from);
    }

    /// The entry of `family` best for a regular, bold, italic or bold
    /// italic face: that style, the weight closest to 400 or 700; among
    /// equals the first.
    fn best_face(&self, family: &str, bold: bool, italic: bool, loaded: bool) -> Option<usize> {
        let target: i32 = if bold { 700 } else { 400 };
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.family.as_deref() == Some(family)
                    && e.bold == bold
                    && e.italic == italic
                    && (!loaded || e.font.is_some())
            })
            .min_by_key(|(_, e)| (i32::from(e.weight) - target).abs())
            .map(|(index, _)| index)
    }

    /// Loads the files of the chosen family's faces (one per style) and
    /// frees all others; returns the files that could not be loaded.
    pub fn load(&mut self, storage: &dyn Storage) -> Vec<String> {
        let mut failed = Vec::new();
        let chosen = self.family.clone();
        let wanted: Vec<usize> = chosen
            .as_deref()
            .map(|family| {
                [(false, false), (true, false), (false, true), (true, true)]
                    .iter()
                    .filter_map(|&(b, i)| self.best_face(family, b, i, false))
                    .collect()
            })
            .unwrap_or_default();
        for (index, entry) in self.entries.iter_mut().enumerate() {
            let Some(path) = &entry.file else {
                continue;
            };
            if !wanted.contains(&index) {
                entry.font = None;
                continue;
            }
            if entry.font.is_some() {
                continue;
            }
            match read_font(storage, path) {
                Some(font) => entry.font = Some(font),
                None => failed.push(path.clone()),
            }
        }
        failed
    }

    /// A font by id, if loaded.
    pub fn get(&self, id: &str) -> Option<FontHandle<'_>> {
        self.index_of(id).map(|index| self.handle(index))
    }

    /// The chosen family's face for a reading role: `None` if no family
    /// is chosen or the role is not a reading role; `Some(None)` if the
    /// family lacks that face.
    fn family_face(&self, role: &str) -> Option<Option<FontHandle<'_>>> {
        let family = self.family.as_deref()?;
        let (bold, italic) = match role {
            r if r == roles::READING.as_str() => (false, false),
            r if r == roles::READING_BOLD.as_str() => (true, false),
            r if r == roles::READING_ITALIC.as_str() => (false, true),
            r if r == roles::READING_BOLD_ITALIC.as_str() => (true, true),
            _ => return None,
        };
        let index = self.best_face(family, bold, italic, true);
        Some(index.map(|index| self.handle(index)))
    }

    fn proposed(&self, role: &str) -> Option<FontHandle<'_>> {
        self.candidates
            .iter()
            .filter(|(r, font, _)| r.as_str() == role && self.get(font.as_str()).is_some())
            .max_by_key(|(_, _, priority)| *priority)
            .and_then(|(_, font, _)| self.get(font.as_str()))
    }

    /// The font bound to `role`: the user's choice, else the chosen
    /// family's face, else the best proposal, else the first font.
    pub fn role(&self, role: &str) -> Option<FontHandle<'_>> {
        let user = self.user.get(role).and_then(|id| self.get(id.as_str()));
        let first = || {
            self.entries
                .iter()
                .position(|e| e.font.is_some())
                .map(|index| self.handle(index))
        };
        user.or_else(|| self.family_face(role).flatten())
            .or_else(|| self.proposed(role))
            .or_else(first)
    }

    /// The font bound to `role` by the user, a chosen family or a plugin;
    /// unlike [`Fonts::role`], no fallback, for optional roles such as bold
    /// faces (a chosen family without bold has none, so its regular face
    /// is used rather than another family's bold).
    pub fn bound(&self, role: &str) -> Option<FontHandle<'_>> {
        let user = self.user.get(role).and_then(|id| self.get(id.as_str()));
        user.or_else(|| match self.family_face(role) {
            Some(face) => face,
            None => self.proposed(role),
        })
    }

    /// Whether the user chose the font for `role` in the configuration.
    pub fn user_bound(&self, role: &str) -> bool {
        self.user.contains_key(role)
    }

    /// Bytes used by the glyph cache.
    pub fn cache_bytes(&self) -> usize {
        self.cache.borrow().bytes()
    }

    /// A handle of loaded entry `index`, with the fallback font for
    /// characters it lacks.
    fn handle(&self, index: usize) -> FontHandle<'_> {
        let entry = &self.entries[index];
        let fallback = self
            .fallback_index()
            .filter(|&f| f != index)
            .and_then(|f| Some((self.entries[f].font.as_ref()?, f as u16)));
        FontHandle {
            id: &entry.id,
            // Handles are made for loaded fonts only.
            font: entry.font.as_ref().unwrap_or_else(|| unreachable!()),
            cache: &self.cache,
            index: index as u16,
            fallback,
        }
    }

    /// The font for characters another one lacks (e.g. Cyrillic in a UI
    /// font for Latin): the one plugins propose for reading, as reading
    /// fonts cover the most scripts.
    fn fallback_index(&self) -> Option<usize> {
        let reading = roles::READING;
        let reading = reading.as_str();
        self.candidates
            .iter()
            .filter(|(r, font, _)| r.as_str() == reading && self.index_of(font.as_str()).is_some())
            .max_by_key(|(_, _, priority)| *priority)
            .and_then(|(_, font, _)| self.index_of(font.as_str()))
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.id.as_str() == id && e.font.is_some())
    }
}

/// Reads a font file into memory.
fn read_font(storage: &dyn Storage, path: &str) -> Option<Font> {
    let size = storage.size(path).ok()?;
    if size > MAX_FONT_FILE {
        return None;
    }
    let mut data = alloc::vec![0u8; size as usize];
    let mut at = 0;
    while at < data.len() {
        let n = storage.read_at(path, at as u64, &mut data[at..]).ok()?;
        if n == 0 {
            return None;
        }
        at += n;
    }
    Font::from_data(data, &[]).ok()
}

/// Fonts a plugin loads itself, e.g. the fonts embedded in a book, with
/// a glyph cache of their own; freed when the set is dropped.
pub struct FontSet {
    fonts: Vec<(Id, Font)>,
    cache: RefCell<GlyphCache>,
}

impl fmt::Debug for FontSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontSet")
            .field(
                "fonts",
                &self.fonts.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl FontSet {
    /// `cache_bytes` bounds the memory for the set's glyphs.
    pub fn new(cache_bytes: usize) -> Self {
        Self {
            fonts: Vec::new(),
            cache: RefCell::new(GlyphCache::new(cache_bytes)),
        }
    }

    /// Adds a font; returns its index.
    pub fn add(&mut self, id: Id, font: Font) -> usize {
        self.fonts.push((id, font));
        self.fonts.len() - 1
    }

    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Font `index`, ready to measure and draw.
    pub fn get(&self, index: usize) -> Option<FontHandle<'_>> {
        let (id, font) = self.fonts.get(index)?;
        Some(FontHandle {
            id,
            font,
            cache: &self.cache,
            index: index as u16,
            fallback: None,
        })
    }
}

/// A font ready to measure and draw text.
#[derive(Clone, Copy)]
pub struct FontHandle<'a> {
    id: &'a Id,
    font: &'a Font,
    /// The glyph cache of the registry or set the font is in.
    cache: &'a RefCell<GlyphCache>,
    /// The font's number in that cache.
    index: u16,
    /// A font for the characters this one lacks, and its number in the
    /// same cache.
    fallback: Option<(&'a Font, u16)>,
}

impl fmt::Debug for FontHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FontHandle").field(self.id()).finish()
    }
}

impl<'a> FontHandle<'a> {
    pub fn id(&self) -> &'a Id {
        self.id
    }

    fn font(&self) -> &'a Font {
        self.font
    }

    /// Ascent, descent and line height at `size` pixels.
    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        self.font().line_metrics(size)
    }

    /// The font that draws `ch` and its number in the cache: this one,
    /// or the fallback if only that has it.
    fn pick(&self, ch: char) -> (&'a Font, u16) {
        match self.fallback {
            Some((font, index)) if !self.font.has_char(ch) && font.has_char(ch) => (font, index),
            _ => (self.font, self.index),
        }
    }

    /// Horizontal advance of one character in pixels.
    pub fn advance(&self, size: f32, ch: char) -> f32 {
        self.pick(ch).0.advance(ch, size)
    }

    /// Width of a single line of text in pixels.
    pub fn measure(&self, size: f32, text: &str) -> f32 {
        match self.fallback {
            None => quireink_text::measure(self.font(), size, text),
            Some(_) => text.chars().map(|ch| self.advance(size, ch)).sum(),
        }
    }

    /// Breaks `text` into lines that fit `max_width`.
    pub fn wrap(&self, size: f32, text: &str, max_width: f32) -> Vec<Range<usize>> {
        quireink_text::wrap(text, max_width, |s| self.measure(size, s))
    }

    /// Draws one line with its baseline at `baseline`; returns its width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        size: f32,
        x: f32,
        baseline: i32,
        text: &str,
        color: Color,
    ) -> f32 {
        self.draw_bolder(canvas, size, x, baseline, text, color, 0)
    }

    /// Like [`FontHandle::draw`], with glyphs widened by `extra` pixels
    /// (e.g. a reader's font weight setting).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_bolder(
        &self,
        canvas: &mut Canvas<'_>,
        size: f32,
        x: f32,
        baseline: i32,
        text: &str,
        color: Color,
        extra: u16,
    ) -> f32 {
        let mut cache = self.cache.borrow_mut();
        let mut pen = x;
        // Runs of characters drawn by the same font.
        let mut rest = text;
        while let Some(first) = rest.chars().next() {
            let (font, index) = self.pick(first);
            let end = rest
                .char_indices()
                .find(|(_, ch)| self.pick(*ch).1 != index)
                .map_or(rest.len(), |(at, _)| at);
            pen += quireink_text::draw_text_bolder(
                canvas,
                &mut cache,
                index,
                font,
                size,
                pen,
                baseline,
                &rest[..end],
                color,
                extra,
            );
            rest = &rest[end..];
        }
        pen - x
    }
}
