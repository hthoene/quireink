//! Text filters: a stage between a book's format and the layout of its
//! pages. Each filter gets every block before it is laid out and may
//! change it: style parts of it (e.g. bold word beginnings), replace text
//! ([`Block::replace`](crate::document::Block::replace) keeps positions in
//! the book right), or leave it.
//!
//! Plugins add filters with
//! [`Registrar::add_text_filter`](crate::Registrar::add_text_filter); a
//! filter decides from the settings whether it does anything. Its
//! signature tells readers when its effect changed, so pages are laid out
//! anew (and kept apart from pages laid out without it).

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::document::Block;
use crate::env::Env;
use crate::id::Id;

/// Changes a block before it is laid out.
pub type TextFilterFn = Box<dyn Fn(&Env<'_>, &mut Block)>;
/// What a filter does now, as text: empty while it does nothing.
pub type TextFilterSignatureFn = Box<dyn Fn(&Env<'_>) -> String>;

/// A text filter contribution.
pub struct TextFilterSpec {
    pub id: Id,
    /// Translation key of its name.
    pub title: Cow<'static, str>,
    pub filter: TextFilterFn,
    pub signature: TextFilterSignatureFn,
}

impl fmt::Debug for TextFilterSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextFilterSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// The registered text filters.
#[derive(Debug, Default)]
pub struct TextFilters {
    filters: Vec<TextFilterSpec>,
}

impl TextFilters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, filter: TextFilterSpec) {
        self.filters.push(filter);
    }

    pub fn shrink_to_fit(&mut self) {
        self.filters.shrink_to_fit();
    }

    /// Runs every filter that does something on `block`.
    pub fn apply(&self, env: &Env<'_>, block: &mut Block) {
        for filter in &self.filters {
            (filter.filter)(env, block);
        }
    }

    /// What the filters do now together; empty if none does anything.
    pub fn signature(&self, env: &Env<'_>) -> String {
        let mut out = String::new();
        for filter in &self.filters {
            let part = (filter.signature)(env);
            if !part.is_empty() {
                out.push_str(filter.id.as_str());
                out.push('=');
                out.push_str(&part);
                out.push(';');
            }
        }
        out
    }
}
