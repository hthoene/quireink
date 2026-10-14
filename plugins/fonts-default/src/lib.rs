//! QuireInk's default fonts, contributed like any other plugin:
//!
//! - **Literata** for reading (`fonts-default.literata`), designed for long
//!   texts on screens, with bold, italic and bold italic faces.
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
pub const LITERATA_BOLD: Id = Id::new_static("fonts-default.literata-bold");
pub const LITERATA_ITALIC: Id = Id::new_static("fonts-default.literata-italic");
pub const LITERATA_BOLD_ITALIC: Id = Id::new_static("fonts-default.literata-bold-italic");
pub const ATKINSON: Id = Id::new_static("fonts-default.atkinson");

/// The faces: id, role, font data and weight (for variable fonts).
type Faces = [(Id, Id, &'static [u8], f32); 5];

#[cfg(feature = "outline")]
mod outlines {
    const LITERATA: &[u8] = include_bytes!("../fonts/Literata.ttf");
    const LITERATA_ITALIC: &[u8] = include_bytes!("../fonts/Literata-Italic.ttf");
    const ATKINSON: &[u8] = include_bytes!("../fonts/AtkinsonHyperlegibleNext.ttf");

    /// Bold faces are instances of the variable fonts.
    pub fn faces() -> super::Faces {
        use super::*;
        [
            (LITERATA, fonts::READING, self::LITERATA, 400.0),
            (LITERATA_BOLD, fonts::READING_BOLD, self::LITERATA, 700.0),
            (
                LITERATA_ITALIC,
                fonts::READING_ITALIC,
                self::LITERATA_ITALIC,
                400.0,
            ),
            (
                LITERATA_BOLD_ITALIC,
                fonts::READING_BOLD_ITALIC,
                self::LITERATA_ITALIC,
                700.0,
            ),
            (ATKINSON, fonts::UI, self::ATKINSON, 400.0),
        ]
    }
}

#[cfg(any(feature = "packs", not(feature = "outline")))]
mod packs {
    macro_rules! pack {
        ($name:literal) => {
            include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".qifp"))
        };
    }

    pub fn faces() -> super::Faces {
        use super::*;
        [
            (LITERATA, fonts::READING, pack!("literata"), 400.0),
            (
                LITERATA_BOLD,
                fonts::READING_BOLD,
                pack!("literata-bold"),
                700.0,
            ),
            (
                LITERATA_ITALIC,
                fonts::READING_ITALIC,
                pack!("literata-italic"),
                400.0,
            ),
            (
                LITERATA_BOLD_ITALIC,
                fonts::READING_BOLD_ITALIC,
                pack!("literata-bold-italic"),
                700.0,
            ),
            (ATKINSON, fonts::UI, pack!("atkinson"), 400.0),
        ]
    }
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
        description: Cow::Borrowed("Literata for reading and Atkinson Hyperlegible for menus."),
        version: Version::new(0, 1, 0),
        sdk: SDK_VERSION,
    }
}

fn register(r: &mut dyn Registrar, faces: Faces) -> Result<(), RegisterError> {
    r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
    for (id, role, data, weight) in faces {
        let font =
            Font::from_data(data, &[("wght", weight)]).map_err(|e| RegisterError::Invalid {
                message: Cow::Owned(alloc::format!("{id}: {e}")),
            })?;
        let family = if role == fonts::UI {
            "Atkinson Hyperlegible"
        } else {
            "Literata"
        };
        let italic = role == fonts::READING_ITALIC || role == fonts::READING_BOLD_ITALIC;
        r.add_font(FontSpec::new(id.clone(), font).family(family, weight >= 600.0, italic))?;
        r.bind_font(role, id, Priority::DEFAULT)?;
    }
    Ok(())
}

impl Plugin for DefaultFonts {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        register(r, preferred::faces())
    }
}

#[cfg(any(feature = "packs", not(feature = "outline")))]
impl Plugin for DefaultFontPacks {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        register(r, packs::faces())
    }
}
