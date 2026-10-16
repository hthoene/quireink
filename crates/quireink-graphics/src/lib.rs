//! Framebuffers and drawing primitives for QuireInk.
//!
//! A [`Framebuffer`] stores pixels in the device's native [`PixelFormat`], so
//! a 800×480 1-bit screen needs 48 KB instead of 384 KB. Drawing goes through
//! a [`Canvas`], which clips to a region and records which part of the screen
//! changed, so the kernel can refresh only that part of the e-ink panel.

#![no_std]

extern crate alloc;

mod canvas;
mod framebuffer;

pub use canvas::{Canvas, Described, Role};
pub use framebuffer::Framebuffer;

use core::fmt;
use quireink_platform::PixelFormat;

/// Errors of this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphicsError {
    /// Not enough memory for the requested buffer.
    OutOfMemory { bytes: usize },
    /// The pixel format is not compiled in (see the `color` feature).
    UnsupportedFormat(PixelFormat),
}

impl fmt::Display for GraphicsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphicsError::OutOfMemory { bytes } => {
                write!(f, "out of memory allocating {bytes} bytes")
            }
            GraphicsError::UnsupportedFormat(format) => {
                write!(f, "pixel format {format:?} is not supported by this build")
            }
        }
    }
}

impl core::error::Error for GraphicsError {}
