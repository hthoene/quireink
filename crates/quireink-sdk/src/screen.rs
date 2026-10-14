use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use quireink_graphics::Canvas;
use quireink_platform::{Instant, PointerEvent, RefreshMode};

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
/// whenever a redraw is requested. Event handlers may also draw directly for
/// small, partial updates; the kernel refreshes exactly the changed region.
pub trait Screen {
    /// Draws the whole screen. The canvas has been cleared to white.
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>);

    /// Reacts to an action. Unhandled actions go to the action's global
    /// handler, if it has one.
    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled;

    /// Reacts to a recognized gesture (tap, long press, swipe). Unhandled
    /// gestures are resolved through the key map (tap zones, swipes).
    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let _ = (gesture, cx);
        Handled::No
    }

    /// Raw pointer samples, offered before gesture recognition. Returning
    /// [`Handled::Yes`] for a `Down` event captures that pointer: all its
    /// further samples go here and no gesture is recognized. Drawing and
    /// handwriting screens use this.
    fn on_pointer(&mut self, event: &PointerEvent, cx: &mut Cx<'_, '_>) -> Handled {
        let _ = (event, cx);
        Handled::No
    }

    /// Called whenever the screen becomes the visible one — after it was
    /// opened and when the screens above it were closed. A good place to
    /// start background work with [`Cx::wake_at`].
    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        let _ = cx;
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
    canvas: Canvas<'a>,
    output: &'a mut Output,
}

impl<'a, 'e> Cx<'a, 'e> {
    /// Used by the kernel and by tests.
    pub fn new(env: &'a Env<'e>, canvas: Canvas<'a>, output: &'a mut Output) -> Self {
        Self {
            env,
            canvas,
            output,
        }
    }

    pub fn env(&self) -> &Env<'e> {
        self.env
    }

    /// Draw here for partial updates.
    pub fn canvas(&mut self) -> &mut Canvas<'a> {
        &mut self.canvas
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

    /// Requests a call to [`Screen::on_timer`] at `at` (the earliest
    /// request wins). Use `cx.env().now` for "as soon as possible".
    pub fn wake_at(&mut self, at: Instant) {
        self.output.wake_at = Some(self.output.wake_at.map_or(at, |w| w.min(at)));
    }
}
