use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::id::Id;
use crate::plugin::RegisterError;

/// A setting's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Text(Cow<'static, str>),
}

/// The type and valid range of a setting. Settings UIs are generated from it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettingKind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    /// One of a fixed set of values.
    Choice(Vec<Cow<'static, str>>),
    Text,
    /// A UI language tag; settings UIs offer the installed translations.
    Language,
}

impl SettingKind {
    /// Parses a value as written in a configuration file.
    pub fn parse(&self, text: &str) -> Option<SettingValue> {
        let value = match self {
            SettingKind::Bool => SettingValue::Bool(match text {
                "true" | "yes" | "on" | "1" => true,
                "false" | "no" | "off" | "0" => false,
                _ => return None,
            }),
            SettingKind::Int { .. } => SettingValue::Int(text.parse().ok()?),
            SettingKind::Choice(_) | SettingKind::Text | SettingKind::Language => {
                SettingValue::Text(Cow::Owned(String::from(text)))
            }
        };
        self.accepts(&value).then_some(value)
    }

    pub fn accepts(&self, value: &SettingValue) -> bool {
        match (self, value) {
            (SettingKind::Bool, SettingValue::Bool(_)) => true,
            (SettingKind::Int { min, max }, SettingValue::Int(v)) => (min..=max).contains(&v),
            (SettingKind::Choice(options), SettingValue::Text(v)) => options.contains(v),
            (SettingKind::Text, SettingValue::Text(_)) => true,
            (SettingKind::Language, SettingValue::Text(v)) => {
                !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }
            _ => false,
        }
    }
}

impl SettingValue {
    /// The value as written in configuration files.
    pub fn to_config_text(&self) -> String {
        match self {
            SettingValue::Bool(true) => String::from("on"),
            SettingValue::Bool(false) => String::from("off"),
            SettingValue::Int(v) => alloc::format!("{v}"),
            SettingValue::Text(v) => String::from(v.as_ref()),
        }
    }
}

/// A setting contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSpec {
    /// `<plugin>.<name>`; in configuration files written as `[<plugin>] <name> = …`.
    pub id: Id,
    /// Translation key of the setting's display name.
    pub title: Cow<'static, str>,
    pub kind: SettingKind,
    pub default: SettingValue,
}

/// Why a setting could not be changed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettingError {
    Unknown,
    InvalidValue,
}

impl fmt::Display for SettingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SettingError::Unknown => "unknown setting",
            SettingError::InvalidValue => "invalid value for this setting",
        })
    }
}

impl core::error::Error for SettingError {}

/// All registered settings and their current values.
///
/// Plugins receive it read-only through [`Env`](crate::Env) and change values
/// with [`Command::SetSetting`](crate::Command::SetSetting).
#[derive(Debug, Default)]
pub struct Settings {
    specs: BTreeMap<Id, SettingSpec>,
    values: BTreeMap<Id, SettingValue>,
    /// Ids in registration order, for settings UIs.
    order: Vec<Id>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a setting. Its default must be valid.
    pub fn define(&mut self, spec: SettingSpec) -> Result<(), RegisterError> {
        if self.specs.contains_key(&spec.id) {
            return Err(RegisterError::Duplicate { id: spec.id });
        }
        if !spec.kind.accepts(&spec.default) {
            return Err(RegisterError::Invalid {
                message: Cow::Borrowed("default value does not match the setting kind"),
            });
        }
        self.order.push(spec.id.clone());
        self.specs.insert(spec.id.clone(), spec);
        Ok(())
    }

    /// Sets a value after validating it.
    pub fn set(&mut self, id: &str, value: SettingValue) -> Result<(), SettingError> {
        let spec = self.specs.get(id).ok_or(SettingError::Unknown)?;
        if !spec.kind.accepts(&value) {
            return Err(SettingError::InvalidValue);
        }
        self.values.insert(spec.id.clone(), value);
        Ok(())
    }

    /// Sets a value from configuration-file text.
    pub fn set_from_text(&mut self, id: &str, text: &str) -> Result<(), SettingError> {
        let spec = self.specs.get(id).ok_or(SettingError::Unknown)?;
        let value = spec.kind.parse(text).ok_or(SettingError::InvalidValue)?;
        self.values.insert(spec.id.clone(), value);
        Ok(())
    }

    /// The current value, or the default if none was set.
    pub fn get(&self, id: &str) -> Option<&SettingValue> {
        self.values
            .get(id)
            .or_else(|| self.specs.get(id).map(|spec| &spec.default))
    }

    /// The value of a boolean setting; `false` if unknown.
    pub fn bool(&self, id: &str) -> bool {
        matches!(self.get(id), Some(SettingValue::Bool(true)))
    }

    /// The value of an integer setting; `0` if unknown.
    pub fn int(&self, id: &str) -> i64 {
        match self.get(id) {
            Some(SettingValue::Int(v)) => *v,
            _ => 0,
        }
    }

    /// The value of a text or choice setting; `""` if unknown.
    pub fn text(&self, id: &str) -> &str {
        match self.get(id) {
            Some(SettingValue::Text(v)) => v,
            _ => "",
        }
    }

    /// All settings in registration order: plugins registered first come
    /// first, each in the order it declared its settings.
    pub fn specs(&self) -> impl Iterator<Item = &SettingSpec> {
        self.order.iter().filter_map(|id| self.specs.get(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let mut settings = Settings::new();
        settings
            .define(SettingSpec {
                id: Id::new_static("test.count"),
                title: Cow::Borrowed("test.count"),
                kind: SettingKind::Int { min: 1, max: 10 },
                default: SettingValue::Int(3),
            })
            .unwrap();
        settings
            .define(SettingSpec {
                id: Id::new_static("test.mode"),
                title: Cow::Borrowed("test.mode"),
                kind: SettingKind::Choice(alloc::vec![Cow::Borrowed("a"), Cow::Borrowed("b")]),
                default: SettingValue::Text(Cow::Borrowed("a")),
            })
            .unwrap();
        settings
    }

    #[test]
    fn defaults_and_updates() {
        let mut s = settings();
        assert_eq!(s.int("test.count"), 3);
        s.set_from_text("test.count", "7").unwrap();
        assert_eq!(s.int("test.count"), 7);
        assert_eq!(s.text("test.mode"), "a");
        s.set_from_text("test.mode", "b").unwrap();
        assert_eq!(s.text("test.mode"), "b");
    }

    #[test]
    fn rejects_invalid_values() {
        let mut s = settings();
        assert_eq!(
            s.set_from_text("test.count", "11"),
            Err(SettingError::InvalidValue)
        );
        assert_eq!(
            s.set_from_text("test.mode", "c"),
            Err(SettingError::InvalidValue)
        );
        assert_eq!(
            s.set("test.count", SettingValue::Bool(true)),
            Err(SettingError::InvalidValue)
        );
        assert_eq!(
            s.set_from_text("test.nope", "1"),
            Err(SettingError::Unknown)
        );
    }

    #[test]
    fn rejects_duplicates_and_bad_defaults() {
        let mut s = settings();
        let spec = SettingSpec {
            id: Id::new_static("test.count"),
            title: Cow::Borrowed(""),
            kind: SettingKind::Bool,
            default: SettingValue::Bool(false),
        };
        assert!(matches!(
            s.define(spec.clone()),
            Err(RegisterError::Duplicate { .. })
        ));
        let bad = SettingSpec {
            id: Id::new_static("test.bad"),
            default: SettingValue::Int(1),
            ..spec
        };
        assert!(matches!(s.define(bad), Err(RegisterError::Invalid { .. })));
    }
}
