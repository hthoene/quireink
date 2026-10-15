use core::fmt;

/// A semantic version (`major.minor.patch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

/// The SDK version implemented by this crate. Plugins declare the version
/// they were built against in their [`Manifest`](crate::Manifest).
pub const SDK_VERSION: Version = Version::new(0, 1, 1);

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses `1`, `1.2` or `1.2.3`.
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version::new(major, minor, patch))
    }

    /// Whether code requiring `required` works with this version, following
    /// SemVer (for `0.x`, the minor version must match).
    pub fn satisfies(&self, required: &Version) -> bool {
        let same_line = if required.major == 0 {
            self.major == 0 && self.minor == required.minor
        } else {
            self.major == required.major
        };
        same_line && self >= required
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_version_matches_crate_version() {
        assert_eq!(Version::parse(env!("CARGO_PKG_VERSION")), Some(SDK_VERSION));
    }

    #[test]
    fn parses_short_forms() {
        assert_eq!(Version::parse("2"), Some(Version::new(2, 0, 0)));
        assert_eq!(Version::parse("0.3"), Some(Version::new(0, 3, 0)));
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("x"), None);
    }

    #[test]
    fn semver_compatibility() {
        let v = Version::new(1, 4, 2);
        assert!(v.satisfies(&Version::new(1, 0, 0)));
        assert!(v.satisfies(&Version::new(1, 4, 2)));
        assert!(!v.satisfies(&Version::new(1, 5, 0)));
        assert!(!v.satisfies(&Version::new(2, 0, 0)));

        let pre = Version::new(0, 2, 5);
        assert!(pre.satisfies(&Version::new(0, 2, 0)));
        assert!(!pre.satisfies(&Version::new(0, 1, 0)));
    }
}
