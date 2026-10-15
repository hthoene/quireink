//! Widgets: small boxes plugins contribute for screens that show them,
//! such as the home screen (a clock, today's reading, a goal). Any plugin
//! adds widgets with
//! [`Registrar::add_widget`](crate::Registrar::add_widget); screens let
//! the user choose which to show.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use quireink_graphics::Canvas;
use quireink_platform::Rect;

use crate::env::Env;
use crate::id::Id;

/// The height a widget needs for a width, in pixels.
pub type WidgetHeightFn = Box<dyn Fn(&Env<'_>, u16) -> u16>;
/// Draws a widget into its area (cleared before).
pub type WidgetDrawFn = Box<dyn Fn(&mut Canvas<'_>, &Env<'_>, Rect)>;
/// What a widget shows now, as text: screens redraw it when it changes
/// (they look each minute).
pub type WidgetSignatureFn = Box<dyn Fn(&Env<'_>) -> String>;

/// A widget contribution.
pub struct WidgetSpec {
    pub id: Id,
    /// Translation key of its name, e.g. "Clock".
    pub title: Cow<'static, str>,
    pub height: WidgetHeightFn,
    pub draw: WidgetDrawFn,
    /// Without one, a widget is drawn when its screen is.
    pub signature: Option<WidgetSignatureFn>,
}

impl fmt::Debug for WidgetSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// The registered widgets.
#[derive(Debug, Default)]
pub struct Widgets {
    widgets: Vec<WidgetSpec>,
}

impl Widgets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, widget: WidgetSpec) {
        self.widgets.push(widget);
    }

    pub fn shrink_to_fit(&mut self) {
        self.widgets.shrink_to_fit();
    }

    fn get(&self, id: &str) -> Option<&WidgetSpec> {
        self.widgets.iter().find(|w| w.id.as_str() == id)
    }

    /// Every widget: its id and the translation key of its name.
    pub fn iter(&self) -> impl Iterator<Item = (&Id, &str)> {
        self.widgets.iter().map(|w| (&w.id, w.title.as_ref()))
    }

    /// The translation key of the name of widget `id`.
    pub fn title(&self, id: &str) -> Option<&str> {
        self.get(id).map(|w| w.title.as_ref())
    }

    /// The height widget `id` needs at `width`; `None` if there is none.
    pub fn height(&self, id: &str, env: &Env<'_>, width: u16) -> Option<u16> {
        self.get(id).map(|w| (w.height)(env, width))
    }

    /// Draws widget `id` in `area`.
    pub fn draw(&self, id: &str, canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
        if let Some(widget) = self.get(id) {
            (widget.draw)(canvas, env, area);
        }
    }

    /// What widget `id` shows now (see [`WidgetSignatureFn`]).
    pub fn signature(&self, id: &str, env: &Env<'_>) -> Option<String> {
        self.get(id)?.signature.as_ref().map(|s| s(env))
    }
}
