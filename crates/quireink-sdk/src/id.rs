use alloc::borrow::{Borrow, Cow};
use alloc::string::String;
use core::fmt;

/// A namespaced identifier such as `core.home` or `diagnostics.screen`.
///
/// Ids consist of one or more segments of lowercase ASCII letters, digits
/// and `-`, separated by `.`. The first segment is the namespace: the id of
/// the plugin that owns the contribution.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(Cow<'static, str>);

/// Returned when parsing an invalid [`Id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidId;

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ids are lowercase segments of [a-z0-9-] separated by '.'")
    }
}

impl core::error::Error for InvalidId {}

impl Id {
    /// Creates an id from a string literal.
    ///
    /// # Panics
    ///
    /// Panics if `id` is invalid. Use it in `const` items, where the check
    /// happens at compile time.
    pub const fn new_static(id: &'static str) -> Id {
        assert!(is_valid(id), "invalid id");
        Id(Cow::Borrowed(id))
    }

    /// Parses an id at runtime, e.g. from a configuration file.
    pub fn parse(id: &str) -> Result<Id, InvalidId> {
        if is_valid(id) {
            Ok(Id(Cow::Owned(String::from(id))))
        } else {
            Err(InvalidId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first segment, i.e. the owning plugin's id.
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    /// Whether this id belongs to the plugin `plugin`.
    pub fn is_owned_by(&self, plugin: &Id) -> bool {
        self.namespace() == plugin.as_str()
    }
}

impl Borrow<str> for Id {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.0)
    }
}

const fn is_valid(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut segment_start = true;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' {
            if segment_start {
                return false;
            }
            segment_start = true;
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' {
            segment_start = false;
        } else {
            return false;
        }
        i += 1;
    }
    !segment_start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ids() {
        for valid in ["core", "core.home", "my-plugin.screen-2", "a.b.c"] {
            assert!(Id::parse(valid).is_ok(), "{valid}");
        }
        for invalid in ["", ".", "a.", ".a", "a..b", "Core.home", "a b", "a_b"] {
            assert!(Id::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn namespace_and_ownership() {
        const ID: Id = Id::new_static("diagnostics.screen");
        assert_eq!(ID.namespace(), "diagnostics");
        assert!(ID.is_owned_by(&Id::new_static("diagnostics")));
        assert!(!ID.is_owned_by(&Id::new_static("diag")));
    }

    #[test]
    fn static_and_parsed_ids_are_equal() {
        assert_eq!(Id::new_static("core.home"), Id::parse("core.home").unwrap());
    }
}
