//! Filters: stages where plugins look at what passes and may change it.
//!
//! - [Text filters](TextFilterSpec): between a book's format and the
//!   layout of its pages.
//! - [Input filters](InputFilterSpec): every button and finger before
//!   anything else sees it, e.g. to lock the screen, or touch while
//!   reading.
//! - [Refresh filters](RefreshFilterSpec): how the panel is refreshed,
//!   e.g. always in full while reading.
//!
//! Text filters are a stage between a book's format and the layout of its
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
use crate::{InputEvent, Rect, RefreshMode};

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

/// What an input filter does with an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputVerdict {
    /// On to the next filter, then the screens and the key maps.
    Pass,
    /// Taken: nothing else sees it.
    Take,
    /// Taken, and this action runs instead.
    Run(Id),
}

/// Looks at an input before anything else does.
pub type InputFilterFn = Box<dyn Fn(&Env<'_>, &InputEvent) -> InputVerdict>;

/// An input filter contribution: it sees every input (buttons pressed and
/// released, fingers down, moving and up) before screens, gestures and key
/// maps do, in the order the plugins registered. One that takes a finger
/// takes it whole, from down to up; one that takes a button, its release
/// too.
pub struct InputFilterSpec {
    pub id: Id,
    /// Translation key of its name.
    pub title: Cow<'static, str>,
    pub filter: InputFilterFn,
}

impl fmt::Debug for InputFilterSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputFilterSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// The registered input filters.
#[derive(Debug, Default)]
pub struct InputFilters {
    filters: Vec<InputFilterSpec>,
}

impl InputFilters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, filter: InputFilterSpec) {
        self.filters.push(filter);
    }

    pub fn shrink_to_fit(&mut self) {
        self.filters.shrink_to_fit();
    }

    /// What the first filter that does not pass it does with `event`.
    pub fn apply(&self, env: &Env<'_>, event: &InputEvent) -> InputVerdict {
        self.filters
            .iter()
            .map(|filter| (filter.filter)(env, event))
            .find(|verdict| *verdict != InputVerdict::Pass)
            .unwrap_or(InputVerdict::Pass)
    }
}

/// A refresh about to be made, as the kernel decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refresh {
    pub mode: RefreshMode,
    /// What is refreshed.
    pub region: Rect,
    /// The screen shown on top.
    pub screen: Option<Id>,
}

/// Changes how the panel is refreshed: another mode, or `None` to leave it.
pub type RefreshFilterFn = Box<dyn Fn(&Env<'_>, &Refresh) -> Option<RefreshMode>>;

/// A refresh filter contribution: it sees every refresh the kernel
/// decided (after the periodic full refresh) and may change its mode;
/// later filters see the change. A full refresh then covers the whole
/// screen.
pub struct RefreshFilterSpec {
    pub id: Id,
    /// Translation key of its name.
    pub title: Cow<'static, str>,
    pub filter: RefreshFilterFn,
}

impl fmt::Debug for RefreshFilterSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefreshFilterSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// The registered refresh filters.
#[derive(Debug, Default)]
pub struct RefreshFilters {
    filters: Vec<RefreshFilterSpec>,
}

impl RefreshFilters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, filter: RefreshFilterSpec) {
        self.filters.push(filter);
    }

    pub fn shrink_to_fit(&mut self) {
        self.filters.shrink_to_fit();
    }

    /// `refresh` as the filters change it; a full one covers `screen`.
    pub fn apply(&self, env: &Env<'_>, mut refresh: Refresh, screen: Rect) -> Refresh {
        for filter in &self.filters {
            if let Some(mode) = (filter.filter)(env, &refresh) {
                refresh.mode = mode;
                if mode == RefreshMode::Full {
                    refresh.region = screen;
                }
            }
        }
        refresh
    }
}
