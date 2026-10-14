//! The QuireInk plugin SDK.
//!
//! QuireInk is a small kernel plus plugins. Every screen, action, setting,
//! key map and translation — including those QuireInk ships with — is
//! contributed by a plugin through this SDK. Anything a built-in plugin does,
//! a community plugin can do too, and replace it.
//!
//! # Concepts
//!
//! - **Ids** ([`Id`]): every contribution has a namespaced id such as
//!   `diagnostics.screen`. The namespace is the plugin id; `core` is
//!   reserved for the kernel.
//! - **Plugins** ([`Plugin`]): a [`Manifest`] plus a `register` function that
//!   adds contributions through a [`Registrar`].
//! - **Contributions**: actions ([`ActionSpec`]), screens ([`ScreenSpec`]),
//!   slots ([`SlotSpec`]), settings ([`SettingSpec`]), fonts ([`FontSpec`]),
//!   key maps and translations (data in the QuireInk config format).
//! - **Slots**: named UI roles such as [`builtin::slots::HOME`]. Any plugin
//!   may bind a screen to any slot; the user's configuration has the final
//!   word.
//! - **Commands** ([`Command`]): plugins never mutate kernel state directly.
//!   They read through [`Env`] and request changes with commands. This keeps
//!   the API message-based, so the same model works for sandboxed
//!   WebAssembly plugins.

#![no_std]

extern crate alloc;

mod action;
pub mod builtin;
mod command;
mod env;
mod fonts;
mod gesture;
mod id;
mod plugin;
mod screen;
mod settings;
mod version;

pub use action::{ActionHandler, ActionSpec};
pub use command::Command;
pub use env::{Env, Metrics};
pub use fonts::{FontHandle, FontSpec, Fonts};
pub use gesture::{Direction, Gesture, GestureKind};
pub use id::{Id, InvalidId};
pub use plugin::{Manifest, Plugin, Priority, RegisterError, Registrar, SlotSpec};
pub use screen::{Cx, Handled, Output, Screen, ScreenFactory, ScreenSpec};
pub use settings::{SettingError, SettingKind, SettingSpec, SettingValue, Settings};
pub use version::{SDK_VERSION, Version};

pub use quireink_graphics::Canvas;
pub use quireink_i18n::{Arg, Translator};
pub use quireink_platform::{
    Color, DeviceInfo, DisplayInfo, InputCaps, Instant, PixelFormat, Point, PointerEvent,
    PointerPhase, PointerTool, Rect, RefreshMode,
};
pub use quireink_text::{Font, FontError, LineMetrics};
