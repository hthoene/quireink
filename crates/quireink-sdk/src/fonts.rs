use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;
use core::ops::Range;

use quireink_graphics::Canvas;
use quireink_platform::Color;
use quireink_text::{Font, GlyphCache, LineMetrics};

use crate::id::Id;
use crate::plugin::{Priority, RegisterError};

/// A font contribution.
#[derive(Debug, Clone)]
pub struct FontSpec {
    /// `<plugin>.<name>`, e.g. `fonts-default.literata`.
    pub id: Id,
    pub font: Font,
}

/// All registered fonts, the role bindings and the glyph cache.
///
/// Fonts are bound to *roles* such as
/// [`builtin::fonts::UI`](crate::builtin::fonts::UI) — like screens to slots.
/// Plugins propose fonts for roles; the user's `[fonts]` configuration
/// overrides them.
pub struct Fonts {
    fonts: Vec<(Id, Font)>,
    candidates: Vec<(Id, Id, Priority)>,
    user: BTreeMap<Id, Id>,
    cache: RefCell<GlyphCache>,
}

impl fmt::Debug for Fonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fonts")
            .field(
                "fonts",
                &self.fonts.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Fonts {
    /// `cache_bytes` bounds the memory for glyphs rasterized at runtime.
    pub fn new(cache_bytes: usize) -> Self {
        Self {
            fonts: Vec::new(),
            candidates: Vec::new(),
            user: BTreeMap::new(),
            cache: RefCell::new(GlyphCache::new(cache_bytes)),
        }
    }

    pub fn add(&mut self, spec: FontSpec) -> Result<(), RegisterError> {
        if self.fonts.iter().any(|(id, _)| *id == spec.id) {
            return Err(RegisterError::Duplicate { id: spec.id });
        }
        self.fonts.push((spec.id, spec.font));
        Ok(())
    }

    /// Proposes `font` for `role`; the highest priority wins.
    pub fn propose(&mut self, role: Id, font: Id, priority: Priority) {
        self.candidates.push((role, font, priority));
    }

    /// The user's explicit choice for a role.
    pub fn bind_user(&mut self, role: Id, font: Id) {
        self.user.insert(role, font);
    }

    pub fn contains(&self, id: &str) -> bool {
        self.index_of(id).is_some()
    }

    /// Ids of all fonts.
    pub fn ids(&self) -> impl Iterator<Item = &Id> {
        self.fonts.iter().map(|(id, _)| id)
    }

    /// A font by id.
    pub fn get(&self, id: &str) -> Option<FontHandle<'_>> {
        self.index_of(id)
            .map(|index| FontHandle { fonts: self, index })
    }

    /// The font bound to `role`: the user's choice, else the best proposal,
    /// else the first registered font.
    pub fn role(&self, role: &str) -> Option<FontHandle<'_>> {
        let user = self.user.get(role).and_then(|id| self.get(id.as_str()));
        let proposed = || {
            self.candidates
                .iter()
                .filter(|(r, font, _)| r.as_str() == role && self.contains(font.as_str()))
                .max_by_key(|(_, _, priority)| *priority)
                .and_then(|(_, font, _)| self.get(font.as_str()))
        };
        let first = || {
            (!self.fonts.is_empty()).then_some(FontHandle {
                fonts: self,
                index: 0,
            })
        };
        user.or_else(proposed).or_else(first)
    }

    /// Bytes used by the glyph cache.
    pub fn cache_bytes(&self) -> usize {
        self.cache.borrow().bytes()
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.fonts.iter().position(|(font, _)| font.as_str() == id)
    }
}

/// A font ready to measure and draw text.
#[derive(Clone, Copy)]
pub struct FontHandle<'a> {
    fonts: &'a Fonts,
    index: usize,
}

impl fmt::Debug for FontHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FontHandle").field(self.id()).finish()
    }
}

impl<'a> FontHandle<'a> {
    pub fn id(&self) -> &'a Id {
        &self.fonts.fonts[self.index].0
    }

    fn font(&self) -> &'a Font {
        &self.fonts.fonts[self.index].1
    }

    /// Ascent, descent and line height at `size` pixels.
    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        self.font().line_metrics(size)
    }

    /// Width of a single line of text in pixels.
    pub fn measure(&self, size: f32, text: &str) -> f32 {
        quireink_text::measure(self.font(), size, text)
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
        let mut cache = self.fonts.cache.borrow_mut();
        quireink_text::draw_text(
            canvas,
            &mut cache,
            self.index as u16,
            self.font(),
            size,
            x,
            baseline,
            text,
            color,
        )
    }
}
