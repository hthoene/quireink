//! QuireInk's default fonts, contributed like any other plugin:
//!
//! - **Literata** for reading (`fonts-default.literata`), designed for long
//!   texts on screens.
//! - **Atkinson Hyperlegible Next** for the UI (`fonts-default.atkinson`),
//!   designed for maximum legibility.
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
mod data {
    pub const LITERATA: &[u8] = include_bytes!("../fonts/Literata.ttf");
    pub const ATKINSON: &[u8] = include_bytes!("../fonts/AtkinsonHyperlegibleNext.ttf");
}

#[cfg(not(feature = "outline"))]
mod data {
    pub const LITERATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/literata.qifp"));
    pub const ATKINSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/atkinson.qifp"));
}

/// The default fonts plugin.
#[derive(Debug, Default)]
pub struct DefaultFonts;

impl Plugin for DefaultFonts {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Default fonts"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let regular = [("wght", 400.0)];
        for (id, data) in [(LITERATA, data::LITERATA), (ATKINSON, data::ATKINSON)] {
            let font = Font::from_data(data, &regular).map_err(|e| RegisterError::Invalid {
                message: Cow::Owned(alloc::format!("{id}: {e}")),
            })?;
            r.add_font(FontSpec { id, font })?;
        }
        r.bind_font(fonts::READING, LITERATA, Priority::DEFAULT)?;
        r.bind_font(fonts::UI, ATKINSON, Priority::DEFAULT)?;
        Ok(())
    }
}
