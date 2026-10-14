/// CLDR plural categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

impl PluralCategory {
    /// Suffix used in message keys, e.g. `library.books.few`.
    pub const fn id(self) -> &'static str {
        match self {
            PluralCategory::Zero => "zero",
            PluralCategory::One => "one",
            PluralCategory::Two => "two",
            PluralCategory::Few => "few",
            PluralCategory::Many => "many",
            PluralCategory::Other => "other",
        }
    }
}

/// Plural rule families for integer counts, named in catalogs by
/// [`PluralRule::id`].
///
/// To support a new language family, add a variant here and document which
/// languages use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluralRule {
    /// No plural distinction: Chinese, Japanese, Korean, Thai, Vietnamese, …
    Other,
    /// `one` for 1: English, German, Dutch, Swedish, Italian, Spanish, …
    OneOther,
    /// `one` for 0 and 1: French, Brazilian Portuguese, …
    ZeroOneOther,
    /// `one`/`few`/`many`: Russian, Ukrainian, Belarusian.
    EastSlavic,
    /// `one`/`few`/`many`: Polish.
    Polish,
    /// `one`/`few`/`other`: Czech, Slovak.
    CzechSlovak,
}

impl PluralRule {
    pub const fn id(self) -> &'static str {
        match self {
            PluralRule::Other => "other",
            PluralRule::OneOther => "one-other",
            PluralRule::ZeroOneOther => "zero-one-other",
            PluralRule::EastSlavic => "east-slavic",
            PluralRule::Polish => "polish",
            PluralRule::CzechSlovak => "czech-slovak",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            PluralRule::Other,
            PluralRule::OneOther,
            PluralRule::ZeroOneOther,
            PluralRule::EastSlavic,
            PluralRule::Polish,
            PluralRule::CzechSlovak,
        ]
        .into_iter()
        .find(|rule| rule.id() == id)
    }

    /// The category for an integer `count`.
    pub fn category(self, count: i64) -> PluralCategory {
        use PluralCategory::{Few, Many, One, Other};
        let n = count.unsigned_abs();
        let (mod10, mod100) = (n % 10, n % 100);
        match self {
            PluralRule::Other => Other,
            PluralRule::OneOther => {
                if n == 1 {
                    One
                } else {
                    Other
                }
            }
            PluralRule::ZeroOneOther => {
                if n <= 1 {
                    One
                } else {
                    Other
                }
            }
            PluralRule::EastSlavic => {
                if mod10 == 1 && mod100 != 11 {
                    One
                } else if (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                    Few
                } else {
                    Many
                }
            }
            PluralRule::Polish => {
                if n == 1 {
                    One
                } else if (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                    Few
                } else {
                    Many
                }
            }
            PluralRule::CzechSlovak => match n {
                1 => One,
                2..=4 => Few,
                _ => Other,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PluralCategory::*;
    use super::*;

    #[test]
    fn east_slavic() {
        let rule = PluralRule::EastSlavic;
        let got: [PluralCategory; 7] = [1, 2, 5, 11, 21, 22, 112].map(|n| rule.category(n));
        assert_eq!(got, [One, Few, Many, Many, One, Few, Many]);
    }

    #[test]
    fn polish_and_czech() {
        assert_eq!(PluralRule::Polish.category(22), Few);
        assert_eq!(PluralRule::Polish.category(21), Many);
        assert_eq!(PluralRule::CzechSlovak.category(3), Few);
        assert_eq!(PluralRule::CzechSlovak.category(22), Other);
    }

    #[test]
    fn ids_round_trip() {
        for id in [
            "other",
            "one-other",
            "zero-one-other",
            "east-slavic",
            "polish",
            "czech-slovak",
        ] {
            assert_eq!(PluralRule::from_id(id).map(PluralRule::id), Some(id));
        }
    }
}
