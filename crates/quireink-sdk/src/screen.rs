use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use quireink_graphics::Canvas;
use quireink_platform::{Instant, PointerEvent, Rect, RefreshMode};

use crate::command::Command;
use crate::env::Env;
use crate::gesture::Gesture;
use crate::id::Id;
use crate::params::Params;

/// Creates a screen instance when it is opened, with the parameters of the
/// command that opened it (e.g. the path of a book).
pub type ScreenFactory = Box<dyn Fn(&Env<'_>, &Params) -> Box<dyn Screen>>;

/// A screen contribution.
pub struct ScreenSpec {
    pub id: Id,
    /// Translation key of the screen's display name.
    pub title: Cow<'static, str>,
    pub factory: ScreenFactory,
}

impl fmt::Debug for ScreenSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// Whether an event was consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handled {
    Yes,
    No,
}

/// A full-screen UI unit (home, library, reader, …).
///
/// The kernel calls [`Screen::draw`] when the screen becomes visible and
/// whenever a redraw is requested. Event handlers never draw; they request
/// a redraw of the screen or of an area ([`Cx::request_redraw_area`]), and
/// the kernel refreshes exactly that region.
pub trait Screen {
    /// Draws the screen. For full screens the canvas has been cleared to
    /// white; overlays paint their own background.
    ///
    /// The canvas is clipped to the part being drawn. On small devices a
    /// frame is drawn in bands, so `draw` may run several times per frame
    /// for different parts: it must not change state, and it can skip what
    /// lies outside [`Canvas::clip`].
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>);

    /// `None` for a full screen; `Some(region)` makes the screen an
    /// overlay (menu, dialog, popup) drawn above the screens below it.
    /// Tapping outside an overlay closes it.
    fn region(&self, env: &Env<'_>) -> Option<Rect> {
        let _ = env;
        None
    }

    /// Reacts to an action. Unhandled actions go to the action's global
    /// handler, if it has one.
    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled;

    /// Reacts to a recognized gesture (tap, long press, swipe). Unhandled
    /// gestures are resolved through the key map (tap zones, swipes).
    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let _ = (gesture, cx);
        Handled::No
    }

    /// Whether the screen takes the edges for itself now: swipes in from
    /// the edges (home, quick panel, back) wait until it no longer does,
    /// e.g. while things on it are arranged by dragging. Gestures a screen
    /// never wants are better switched off for it (`gestures.policy`).
    fn holds_edges(&self) -> bool {
        false
    }

    /// Raw pointer samples, offered before gesture recognition. Returning
    /// [`Handled::Yes`] for a `Down` event captures that pointer: all its
    /// further samples go here and no gesture is recognized. Drawing and
    /// handwriting screens use this.
    fn on_pointer(&mut self, event: &PointerEvent, cx: &mut Cx<'_, '_>) -> Handled {
        let _ = (event, cx);
        Handled::No
    }

    /// Called whenever the screen becomes the visible one: after it was
    /// opened and when the screens above it were closed. A good place to
    /// start background work with [`Cx::wake_at`].
    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        let _ = cx;
    }

    /// Called for every emitted event (after its subscribers), on every
    /// open screen, e.g. `core.setting-changed` to re-layout.
    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        let _ = (event, params, cx);
    }

    /// Called when a time requested with [`Cx::wake_at`] has come. Long
    /// work (e.g. paginating a book) is split into steps that each request
    /// the next wake-up, so input stays responsive.
    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let _ = cx;
    }
}

/// What a plugin asked the kernel to do during an event.
#[derive(Debug, Default)]
pub struct Output {
    pub commands: Vec<Command>,
    /// Requested refresh mode for the region drawn during the event.
    pub refresh: Option<RefreshMode>,
    /// Whether the whole screen should be redrawn.
    pub redraw: bool,
    /// Part of the screen to redraw, in screen coordinates.
    pub redraw_area: Option<Rect>,
    /// When the active screen wants [`Screen::on_timer`] to be called.
    pub wake_at: Option<Instant>,
}

impl Output {
    /// Records a refresh request; the strongest request wins.
    pub fn request_refresh(&mut self, mode: RefreshMode) {
        self.refresh = Some(self.refresh.map_or(mode, |m| m.max(mode)));
    }
}

/// Context passed to screen event handlers.
#[derive(Debug)]
pub struct Cx<'a, 'e> {
    env: &'a Env<'e>,
    output: &'a mut Output,
}

impl<'a, 'e> Cx<'a, 'e> {
    /// Used by the kernel and by tests.
    pub fn new(env: &'a Env<'e>, output: &'a mut Output) -> Self {
        Self { env, output }
    }

    pub fn env(&self) -> &Env<'e> {
        self.env
    }

    /// Queues a command for the kernel.
    pub fn command(&mut self, command: Command) {
        self.output.commands.push(command);
    }

    /// Requests a refresh mode for what was drawn; the strongest request of
    /// an event wins.
    pub fn request_refresh(&mut self, mode: RefreshMode) {
        self.output.request_refresh(mode);
    }

    /// Requests a complete redraw via [`Screen::draw`].
    pub fn request_redraw(&mut self) {
        self.output.redraw = true;
    }

    /// Requests a redraw of `area` (screen coordinates) only, e.g. a status
    /// line or a focused item: [`Screen::draw`] runs clipped to it.
    pub fn request_redraw_area(&mut self, area: Rect) {
        self.output.redraw_area = Some(self.output.redraw_area.map_or(area, |a| a.union(&area)));
    }

    /// Requests a call to [`Screen::on_timer`] at `at` (the earliest
    /// request wins). Use `cx.env().now` for "as soon as possible".
    pub fn wake_at(&mut self, at: Instant) {
        self.output.wake_at = Some(self.output.wake_at.map_or(at, |w| w.min(at)));
    }
}
