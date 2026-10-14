//! QuireInk's default fonts, contributed like any other plugin:
//!
//! - **Literata** for reading (`fonts-default.literata`), designed for long
//!   texts on screens.
//! - **Atkinson Hyperlegible Next** for the UI (`fonts-default.atkinson`),
//!   designed for maximum legibility.
//!
//! [`DefaultFonts`] ships them as scalable fonts when built with the
//! `outline` feature, else as bitmap packs. [`DefaultFontPacks`] always uses
//! bitmap packs (feature `packs`), e.g. to emulate small devices.
//!
//! Both are under the SIL Open Font License 1.1. Users can bind other fonts
//! to the roles:
//!
//! ```ini
//! [fonts]
//! core.font-reading = my-fonts.garamond
//! ```

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;

use quireink_sdk::builtin::fonts;
use quireink_sdk::{
    Font, FontSpec, Id, Manifest, Plugin, Priority, RegisterError, Registrar, SDK_VERSION, Version,
};

pub const PLUGIN: Id = Id::new_static("fonts-default");
pub const LITERATA: Id = Id::new_static("fonts-default.literata");
pub const ATKINSON: Id = Id::new_static("fonts-default.atkinson");

#[cfg(feature = "outline")]
mod outlines {
    pub const LITERATA: &[u8] = include_bytes!("../fonts/Literata.ttf");
    pub const ATKINSON: &[u8] = include_bytes!("../fonts/AtkinsonHyperlegibleNext.ttf");
}

#[cfg(any(feature = "packs", not(feature = "outline")))]
mod packs {
    pub const LITERATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/literata.qifp"));
    pub const ATKINSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/atkinson.qifp"));
}

#[cfg(feature = "outline")]
use outlines as preferred;
#[cfg(not(feature = "outline"))]
use packs as preferred;

/// The default fonts plugin: scalable fonts if available, else bitmap packs.
#[derive(Debug, Default)]
pub struct DefaultFonts;

/// The default fonts as bitmap packs, read in place without a glyph cache.
#[cfg(any(feature = "packs", not(feature = "outline")))]
#[derive(Debug, Default)]
pub struct DefaultFontPacks;

fn manifest() -> Manifest {
    Manifest {
        id: PLUGIN,
        name: Cow::Borrowed("Default fonts"),
        version: Version::new(0, 1, 0),
        sdk: SDK_VERSION,
    }
}

fn register(
    r: &mut dyn Registrar,
    literata: &'static [u8],
    atkinson: &'static [u8],
) -> Result<(), RegisterError> {
    let regular = [("wght", 400.0)];
    for (id, data) in [(LITERATA, literata), (ATKINSON, atkinson)] {
        let font = Font::from_data(data, &regular).map_err(|e| RegisterError::Invalid {
            message: Cow::Owned(alloc::format!("{id}: {e}")),
        })?;
        r.add_font(FontSpec { id, font })?;
    }
    r.bind_font(fonts::READING, LITERATA, Priority::DEFAULT)?;
    r.bind_font(fonts::UI, ATKINSON, Priority::DEFAULT)?;
    Ok(())
}

impl Plugin for DefaultFonts {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        register(r, preferred::LITERATA, preferred::ATKINSON)
    }
}

#[cfg(any(feature = "packs", not(feature = "outline")))]
impl Plugin for DefaultFontPacks {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        register(r, packs::LITERATA, packs::ATKINSON)
    }
}
