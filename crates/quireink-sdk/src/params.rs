use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

/// Named string parameters passed with commands and events, e.g. the path
/// of the book a reader screen should open.
///
/// Parameters are plain strings so they can cross a message boundary
/// (WebAssembly plugins) unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params(Vec<(Cow<'static, str>, String)>);

impl Params {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Adds or replaces a parameter.
    pub fn with(mut self, key: impl Into<Cow<'static, str>>, value: impl Into<String>) -> Self {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: impl Into<Cow<'static, str>>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_ref(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_replace() {
        let params = Params::new().with("path", "a.txt").with("path", "b.txt");
        assert_eq!(params.get("path"), Some("b.txt"));
        assert_eq!(params.get("other"), None);
        assert_eq!(params.iter().count(), 1);
    }
}
