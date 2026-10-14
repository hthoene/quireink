use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::collections::VecMap;
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
    /// The group it is shown in, e.g.
    /// [`builtin::groups::READING`](crate::builtin::groups::READING).
    pub group: Id,
    pub level: Level,
    /// Translation key of a short help text.
    pub description: Option<Cow<'static, str>>,
    /// Translation key that formats a number with its unit, with plural
    /// forms, e.g. `core.unit.pt.other = {count} pt`.
    pub unit: Option<Cow<'static, str>>,
}

impl SettingSpec {
    /// A basic setting in the [`OTHER`](crate::builtin::groups::OTHER)
    /// group; see the builder methods for the rest.
    pub fn new(
        id: Id,
        title: impl Into<Cow<'static, str>>,
        kind: SettingKind,
        default: SettingValue,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            kind,
            default,
            group: crate::builtin::groups::OTHER,
            level: Level::Basic,
            description: None,
            unit: None,
        }
    }

    pub fn group(mut self, group: Id) -> Self {
        self.group = group;
        self
    }

    /// Shown only with advanced settings.
    pub fn advanced(mut self) -> Self {
        self.level = Level::Advanced;
        self
    }

    pub fn description(mut self, key: impl Into<Cow<'static, str>>) -> Self {
        self.description = Some(key.into());
        self
    }

    pub fn unit(mut self, key: impl Into<Cow<'static, str>>) -> Self {
        self.unit = Some(key.into());
        self
    }
}

/// How often a setting is needed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// What most readers change (font size, language, …).
    #[default]
    Basic,
    /// Rarely needed; shown when the user asks for advanced settings.
    Advanced,
}

/// A group of settings, shown together on one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingGroupSpec {
    pub id: Id,
    /// Translation key of the group's name.
    pub title: Cow<'static, str>,
    /// Groups are listed by order, smallest first.
    pub order: i32,
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
    specs: VecMap<Id, SettingSpec>,
    values: VecMap<Id, SettingValue>,
    /// Ids in registration order, for settings UIs.
    order: Vec<Id>,
    /// Groups by order.
    groups: Vec<SettingGroupSpec>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Releases unused capacity once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.specs.shrink_to_fit();
        self.values.shrink_to_fit();
        self.order.shrink_to_fit();
        self.groups.shrink_to_fit();
    }

    /// Registers a group of settings.
    pub fn add_group(&mut self, group: SettingGroupSpec) -> Result<(), RegisterError> {
        if self.groups.iter().any(|g| g.id == group.id) {
            return Err(RegisterError::Duplicate { id: group.id });
        }
        let at = self
            .groups
            .partition_point(|g| (g.order, g.id.as_str()) <= (group.order, group.id.as_str()));
        self.groups.insert(at, group);
        Ok(())
    }

    /// Groups by order.
    pub fn groups(&self) -> impl Iterator<Item = &SettingGroupSpec> {
        self.groups.iter()
    }

    /// The group a setting is shown in: its own if registered, else
    /// [`OTHER`](crate::builtin::groups::OTHER).
    pub fn group_of(&self, spec: &SettingSpec) -> Id {
        if self.groups.iter().any(|g| g.id == spec.group) {
            spec.group.clone()
        } else {
            crate::builtin::groups::OTHER
        }
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

    /// Replaces the options of a [`SettingKind::Choice`] setting, e.g. once
    /// all contributions it chooses from are known.
    pub fn set_choices(&mut self, id: &str, options: Vec<Cow<'static, str>>) {
        if let Some(spec) = self.specs.get_mut(id) {
            spec.kind = SettingKind::Choice(options);
        }
    }

    /// Changes a setting's default (e.g. device-specific defaults chosen by
    /// a shell). The user's own values still take precedence.
    pub fn set_default_from_text(&mut self, id: &str, text: &str) -> Result<(), SettingError> {
        let spec = self.specs.get_mut(id).ok_or(SettingError::Unknown)?;
        spec.default = spec.kind.parse(text).ok_or(SettingError::InvalidValue)?;
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
            .define(SettingSpec::new(
                Id::new_static("test.count"),
                "test.count",
                SettingKind::Int { min: 1, max: 10 },
                SettingValue::Int(3),
            ))
            .unwrap();
        settings
            .define(SettingSpec::new(
                Id::new_static("test.mode"),
                "test.mode",
                SettingKind::Choice(alloc::vec![Cow::Borrowed("a"), Cow::Borrowed("b")]),
                SettingValue::Text(Cow::Borrowed("a")),
            ))
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
    fn groups_are_ordered_and_unknown_groups_fall_back() {
        let mut s = settings();
        for (id, order) in [("b.late", 20), ("a.early", 10), ("c.same", 10)] {
            s.add_group(SettingGroupSpec {
                id: Id::parse(id).unwrap(),
                title: Cow::Borrowed(""),
                order,
            })
            .unwrap();
        }
        let ids: Vec<&str> = s.groups().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["a.early", "c.same", "b.late"]);
        let spec = SettingSpec::new(
            Id::new_static("x.y"),
            "",
            SettingKind::Bool,
            SettingValue::Bool(false),
        )
        .group(Id::new_static("missing.group"));
        assert_eq!(s.group_of(&spec), crate::builtin::groups::OTHER);
        assert_eq!(
            s.group_of(&spec.clone().group(Id::new_static("b.late")))
                .as_str(),
            "b.late"
        );
    }

    #[test]
    fn rejects_duplicates_and_bad_defaults() {
        let mut s = settings();
        let spec = SettingSpec::new(
            Id::new_static("test.count"),
            "",
            SettingKind::Bool,
            SettingValue::Bool(false),
        );
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
