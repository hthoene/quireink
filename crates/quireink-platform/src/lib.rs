//! Platform vocabulary for QuireInk.
//!
//! This crate is the common language between platform shells and the
//! kernel: display descriptions, pixel formats and raw input events. It has
//! no dependencies and never allocates, so it can be used on every target,
//! down to bare-metal microcontrollers.
//!
//! The kernel itself never calls into a platform. It consumes
//! [`InputEvent`]s and emits effects, which a shell executes, for example
//! with a [`Display`] implementation.

#![no_std]

pub mod display;
pub mod geometry;
pub mod input;
pub mod pixel;
pub mod storage;

pub use display::{ColorPanel, Display, DisplayInfo, FrameView, RefreshMode, Rotation};
pub use geometry::{Point, Rect};
pub use input::{
    ButtonState, InputCaps, InputEvent, Instant, PointerEvent, PointerPhase, PointerTool, buttons,
};
pub use pixel::{Color, PixelFormat};
pub use storage::{DirEntry, NoStorage, Storage, StorageError};

/// Static description of a device, provided by its shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier, e.g. `xteink-x4`.
    pub id: &'static str,
    /// Human-readable model name.
    pub name: &'static str,
    pub display: DisplayInfo,
    pub input: InputCaps,
}
