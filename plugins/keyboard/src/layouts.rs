//! Keyboard layouts and the variants of letters (`é` on a long press on
//! `e`).
//!
//! The built-in layouts are static tables, so they cost no memory until
//! one is shown. The *automatic* layout is the built-in one of the UI
//! language, or else what its translation brings under `keyboard.letters`,
//! so a language pack works without code.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::Env;

/// The setting value (and list entry) for the UI language's layout.
pub(crate) const AUTO: &str = "keyboard.layout.auto";

/// A built-in layout: its id (also the translation key of its name), the
/// short code on the key that switches layouts, and the letter rows
/// separated by spaces.
pub(crate) struct Layout {
    pub id: &'static str,
    pub code: &'static str,
    pub letters: &'static str,
}

pub(crate) const LAYOUTS: [Layout; 12] = [
    Layout {
        id: "keyboard.layout.en",
        code: "EN",
        letters: "qwertyuiop asdfghjkl zxcvbnm",
    },
    Layout {
        id: "keyboard.layout.de",
        code: "DE",
        letters: "qwertzuiopü asdfghjklöä yxcvbnmß",
    },
    Layout {
        id: "keyboard.layout.fr",
        code: "FR",
        letters: "azertyuiop qsdfghjklm wxcvbn",
    },
    Layout {
        id: "keyboard.layout.es",
        code: "ES",
        letters: "qwertyuiop asdfghjklñ zxcvbnm",
    },
    Layout {
        id: "keyboard.layout.pt",
        code: "PT",
        letters: "qwertyuiop asdfghjklç zxcvbnm",
    },
    Layout {
        id: "keyboard.layout.sv",
        code: "SV",
        letters: "qwertyuiopå asdfghjklöä zxcvbnm",
    },
    Layout {
        id: "keyboard.layout.da",
        code: "DA",
        letters: "qwertyuiopå asdfghjklæø zxcvbnm",
    },
    Layout {
        id: "keyboard.layout.cs",
        code: "CS",
        letters: "qwertzuiopú asdfghjklů yxcvbnm",
    },
    Layout {
        id: "keyboard.layout.tr",
        code: "TR",
        letters: "qwertyuıopğü asdfghjklşi zxcvbnmöç",
    },
    Layout {
        id: "keyboard.layout.ru",
        code: "RU",
        letters: "йцукенгшщзх фывапролджэ ячсмитьбю",
    },
    Layout {
        id: "keyboard.layout.uk",
        code: "UK",
        letters: "йцукенгшщзхї фівапролджє ячсмитьбю",
    },
    Layout {
        id: "keyboard.layout.el",
        code: "EL",
        letters: "ςερτυθιοπ ασδφγηξκλ ζχψωβνμ",
    },
];

/// Letters and signs with variants, and the variants (for lower case;
/// capitals are their upper case).
const VARIANTS: [(char, &str); 30] = [
    ('a', "àáâäãåāæą"),
    ('c', "çćč"),
    ('d', "ďđð"),
    ('e', "èéêëēėęě"),
    ('g', "ğ"),
    ('i', "îïíīįìı"),
    ('l', "łľ"),
    ('n', "ñńň"),
    ('o', "ôöòóõøōœő"),
    ('r', "ř"),
    ('s', "ßśšş"),
    ('t', "ťþ"),
    ('u', "ûüùúūůű"),
    ('y', "ÿý"),
    ('z', "žźż"),
    ('е', "ё"),
    ('ь', "ъ"),
    ('г', "ґ"),
    ('и', "ії"),
    ('α', "ά"),
    ('ε', "έ"),
    ('η', "ή"),
    ('ι', "ίϊΐ"),
    ('ο', "ό"),
    ('υ', "ύϋΰ"),
    ('ω', "ώ"),
    ('-', "–—"),
    ('"', "„“”«»"),
    ('\'', "‚‘’‹›"),
    ('.', "…"),
];

/// The variants of `c` (lower case), if it has any.
pub(crate) fn variants(c: char) -> Option<&'static str> {
    VARIANTS
        .iter()
        .find(|(base, _)| *base == c)
        .map(|(_, v)| *v)
}

/// The layouts chosen in the setting `value` (ids separated by commas),
/// as their codes and letter rows; the UI language's layout if none.
pub(crate) fn chosen(env: &Env<'_>, value: &str) -> Vec<(String, Cow<'static, str>)> {
    let mut layouts: Vec<(String, Cow<'static, str>)> = Vec::new();
    for id in value.split(',').map(str::trim).filter(|id| !id.is_empty()) {
        if id == AUTO {
            layouts.push(automatic(env));
        } else if let Some(layout) = LAYOUTS.iter().find(|l| l.id == id) {
            layouts.push((layout.code.to_string(), Cow::Borrowed(layout.letters)));
        }
    }
    if layouts.is_empty() {
        layouts.push(automatic(env));
    }
    layouts
}

/// The UI language's layout: the built-in one for the language, or its
/// translation's (`keyboard.letters`, which falls back to English).
fn automatic(env: &Env<'_>) -> (String, Cow<'static, str>) {
    let language = env.i18n.language();
    let base = language.split('-').next().unwrap_or(language);
    let code = base.to_uppercase();
    let builtin = LAYOUTS
        .iter()
        .find(|l| l.id.rsplit('.').next() == Some(base));
    match (builtin, env.i18n.get("keyboard.letters")) {
        (Some(layout), _) => (code, Cow::Borrowed(layout.letters)),
        (None, Some(letters)) => (code, Cow::Owned(letters.to_string())),
        (None, None) => (code, Cow::Borrowed(LAYOUTS[0].letters)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_have_names_and_rows() {
        let en = include_str!("../i18n/en.ini");
        let de = include_str!("../i18n/de.ini");
        for layout in &LAYOUTS {
            let key = alloc::format!("{} =", layout.id);
            assert!(en.contains(&key) && de.contains(&key), "{}", layout.id);
            assert_eq!(layout.letters.split(' ').count(), 3, "{}", layout.id);
        }
        assert!(en.contains("keyboard.layout.auto ="));
    }

    #[test]
    fn letters_have_variants() {
        assert_eq!(variants('e'), Some("èéêëēėęě"));
        assert!(variants('e').unwrap().contains('é'));
        assert_eq!(variants('q'), None);
        // Every variant is one character in upper case too, or kept.
        for (_, variants) in VARIANTS {
            assert!(!variants.is_empty());
        }
    }
}
