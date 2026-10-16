//! The QuireInk plugin SDK.
//!
//! QuireInk is a small kernel plus plugins. Every screen, action, setting,
//! key map and translation (including those QuireInk ships with) is
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
//!   file formats ([`FormatSpec`]), event subscriptions ([`EventHandler`]),
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
pub mod annotations;
pub mod builtin;
pub mod collections;
mod command;
mod env;
mod event;
pub mod files;
pub mod filters;
mod fonts;
mod formats;
mod gesture;
mod hyphenation;
pub mod icons;
mod id;
pub mod json;
pub mod keys;
pub mod library;
pub mod md5;
pub mod qr;
pub mod secrets;
pub mod semantics;
pub mod sha1;
/// Reading and writing SQLite database files.
pub mod sqlite {
    pub use quireink_sqlite::*;
}
mod memory_storage;
mod menus;
mod params;
mod plugin;
mod plugins;
mod screen;
mod settings;
pub mod status;
pub mod time;
pub mod ui;
mod version;
pub mod web;
pub mod widgets;

pub use action::{ActionHandler, ActionSpec, Actions};
pub use command::{
    Command, HttpRequest, MAX_CLIPBOARD, ServerResponse, SocketRead, Speech, http_params,
    server_params, socket_params, speech_params,
};
pub use env::{Env, Metrics};
pub use event::EventHandler;
pub use fonts::{FontHandle, FontSet, FontSpec, Fonts};
pub use formats::{FormatOpener, FormatSpec, Formats, OpenOptions};
pub use gesture::{Direction, Gesture, GestureKind};
pub use hyphenation::{Hyphenation, HyphenationSpec};
pub use id::{Id, InvalidId};
pub use memory_storage::{MemoryStorage, read_file};
pub use menus::{MenuEntry, MenuItem, MenuNeed, MenuSection, Menus};
pub use params::Params;
pub use plugin::{Manifest, Plugin, Priority, RegisterError, Registrar, SlotSpec};
pub use plugins::{PluginInfo, PluginState, Plugins, RequiredFor};
pub use screen::{Cx, Handled, Output, Screen, ScreenFactory, ScreenSpec};
pub use settings::{
    Level, SettingError, SettingGroupSpec, SettingKind, SettingSpec, SettingValue, Settings,
};
pub use version::{SDK_VERSION, Version};

pub use quireink_graphics::Canvas;
pub use quireink_i18n::{Arg, Translator};
pub use quireink_platform::{Battery, SystemStatus, WallClock};
pub use quireink_platform::{
    Color, DeviceInfo, DisplayInfo, InputCaps, Instant, MemoryClass, PixelFormat, Point,
    PointerEvent, PointerPhase, PointerTool, Rect, RefreshMode,
};
pub use quireink_platform::{DirEntry, NoStorage, Storage, StorageError};
pub use status::{
    StatusFn, StatusFrame, StatusItemSpec, StatusItems, StatusLineFn, StatusLineSpec,
    StatusSignatureFn, StatusValue,
};

/// The human-editable config format (see `quireink-config`), for plugins
/// that store their own data as text files.
pub mod config {
    pub use quireink_config::*;
}

/// The format-independent document model (see `quireink-document`).
pub mod document {
    pub use quireink_document::*;
}

/// Page layout engine (see `quireink-layout`), usable by any reader plugin.
pub mod layout {
    pub use quireink_layout::*;
}

/// Streaming XML/XHTML tokenizer (see `quireink-xml`) for format plugins.
pub mod xml {
    pub use quireink_xml::*;
}

/// Streaming JPEG, PNG and GIF decoding to gray or colour (see
/// `quireink-image`).
pub mod image {
    pub use quireink_image::*;

    use crate::document::ImageRef;
    use quireink_platform::Storage;

    /// Reads a file in storage as an image source.
    struct FileSource<'a> {
        storage: &'a dyn Storage,
        path: &'a str,
        offset: u64,
    }

    impl Source for FileSource<'_> {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, ImageError> {
            let n = self
                .storage
                .read_at(self.path, self.offset, buf)
                .map_err(|_| ImageError::Io)?;
            self.offset += n as u64;
            Ok(n)
        }
    }

    /// Decodes the JPEG or PNG file `from` into the gray file `to`, no side
    /// longer than `side` (as far as the format scales), for
    /// [`ui::draw_image`](crate::ui::draw_image); `None` if it is no image
    /// QuireInk reads.
    pub fn gray_from_file(
        storage: &dyn Storage,
        from: &str,
        to: &str,
        side: u32,
    ) -> Option<ImageRef> {
        picture_from_file(storage, from, to, side, false)
    }

    /// [`gray_from_file`], or in colour (for colour screens) with `color`.
    pub fn picture_from_file(
        storage: &dyn Storage,
        from: &str,
        to: &str,
        side: u32,
        color: bool,
    ) -> Option<ImageRef> {
        let mut head = [0u8; 8];
        let n = storage.read_at(from, 0, &mut head).ok()?;
        let format = Format::detect(&head[..n])?;
        let mut source = FileSource {
            storage,
            path: from,
            offset: 0,
        };
        if color {
            let mut writer = ColorWriter::new(storage, to);
            decode(format, &mut source, side, &mut writer)
                .and_then(|()| writer.finish())
                .ok()?;
        } else {
            let mut writer = GrayWriter::new(storage, to);
            decode(format, &mut source, side, &mut writer)
                .and_then(|()| writer.finish())
                .ok()?;
        }
        ImageFile::open(storage, to).map(|file| ImageRef {
            path: file.path,
            width: file.width,
            height: file.height,
        })
    }
}

/// Hyphenation with compiled patterns (see `quireink-hyphen`).
/// Text and font helpers (see `quireink-text`), e.g. reading a font
/// file's family.
pub mod text {
    pub use quireink_text::{OUTLINES, sfnt};
}

pub mod hyphen {
    pub use quireink_hyphen::*;
}

/// Streaming ZIP reader (see `quireink-zip`) for ZIP-based formats.
pub mod zip {
    pub use quireink_zip::*;
}
pub use quireink_text::{Font, FontError, LineMetrics};
