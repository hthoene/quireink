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
    /// Ids separated by commas, e.g. the status items of a status line;
    /// shown by their names, chosen on the setting's editor screen.
    List,
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
            SettingKind::Choice(_)
            | SettingKind::Text
            | SettingKind::Language
            | SettingKind::List => SettingValue::Text(Cow::Owned(String::from(text))),
        };
        self.accepts(&value).then_some(value)
    }

    pub fn accepts(&self, value: &SettingValue) -> bool {
        match (self, value) {
            (SettingKind::Bool, SettingValue::Bool(_)) => true,
            (SettingKind::Int { min, max }, SettingValue::Int(v)) => (min..=max).contains(&v),
            (SettingKind::Choice(options), SettingValue::Text(v)) => options.contains(v),
            (SettingKind::Text | SettingKind::List, SettingValue::Text(_)) => true,
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
    /// A screen that edits the value, opened with the parameter `id` (the
    /// setting's id), e.g. a folder chooser for a path.
    pub editor: Option<Id>,
    /// May differ per scope, e.g. per book (see [`Settings::scope`]).
    pub scoped: bool,
    /// How much one step changes a number (e.g. 10 for percentages).
    pub step: i64,
    /// A number better set by eye than by its value, such as a light's
    /// brightness: panels show a slider.
    pub slider: bool,
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
            editor: None,
            scoped: false,
            step: 1,
            slider: false,
        }
    }

    /// A number set with a slider in panels (see [`SettingSpec::slider`]).
    pub fn slider(mut self) -> Self {
        self.slider = true;
        self
    }

    /// How much one step (a `+` or `−`) changes a number.
    pub fn step(mut self, step: i64) -> Self {
        self.step = step.max(1);
        self
    }

    /// May be set per scope, e.g. for one book only.
    pub fn scoped(mut self) -> Self {
        self.scoped = true;
        self
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

    /// Edited on its own screen (see [`SettingSpec::editor`]).
    pub fn editor(mut self, screen: Id) -> Self {
        self.editor = Some(screen);
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
    /// The values of the active scope (e.g. the open book), if any.
    scope: Option<VecMap<Id, SettingValue>>,
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

    /// The current value: the active scope's if it sets one (see
    /// [`Settings::scope`]), else the global value, else the default.
    pub fn get(&self, id: &str) -> Option<&SettingValue> {
        self.scoped_value(id).or_else(|| self.global(id))
    }

    /// The value outside any scope, or the default if none was set.
    pub fn global(&self, id: &str) -> Option<&SettingValue> {
        self.values
            .get(id)
            .or_else(|| self.specs.get(id).map(|spec| &spec.default))
    }

    /// The active scope's value of `id`, if it applies: the setting is
    /// [`scoped`](SettingSpec::scoped) and the scope is switched on
    /// ([`SCOPE_ONLY`](crate::builtin::settings::SCOPE_ONLY)).
    pub fn scoped_value(&self, id: &str) -> Option<&SettingValue> {
        let scope = self.scope.as_ref()?;
        let only = crate::builtin::settings::SCOPE_ONLY;
        if id != only.as_str() && scope.get(only.as_str()) != Some(&SettingValue::Bool(true)) {
            return None;
        }
        self.specs.get(id).filter(|spec| spec.scoped)?;
        scope.get(id)
    }

    /// The values of the active scope, e.g. of the book being read: scoped
    /// settings changed while it is switched on are kept there. `None`
    /// outside any scope.
    pub fn scope(&self) -> Option<&VecMap<Id, SettingValue>> {
        self.scope.as_ref()
    }

    /// Activates a scope with its values (only valid scoped ones are
    /// kept), or ends it.
    pub fn set_scope(&mut self, values: Option<VecMap<Id, SettingValue>>) {
        self.scope = values.map(|mut values| {
            let ids: Vec<Id> = values.keys().cloned().collect();
            for id in ids {
                let valid = match (self.specs.get(id.as_str()), values.get(id.as_str())) {
                    (Some(spec), Some(value)) => spec.scoped && spec.kind.accepts(value),
                    _ => false,
                };
                if !valid {
                    values.remove(id.as_str());
                }
            }
            values
        });
    }

    /// Sets a scoped setting in the active scope. Switching the scope off
    /// ([`SCOPE_ONLY`](crate::builtin::settings::SCOPE_ONLY)) drops its
    /// values.
    pub fn set_in_scope(&mut self, id: &str, value: SettingValue) -> Result<(), SettingError> {
        let spec = self.specs.get(id).ok_or(SettingError::Unknown)?;
        if !spec.scoped || !spec.kind.accepts(&value) {
            return Err(SettingError::InvalidValue);
        }
        let id = spec.id.clone();
        let scope = self.scope.as_mut().ok_or(SettingError::Unknown)?;
        if id == crate::builtin::settings::SCOPE_ONLY && value == SettingValue::Bool(false) {
            scope.clear();
            return Ok(());
        }
        scope.insert(id, value);
        Ok(())
    }

    /// Whether `id` is set in the active scope rather than globally, e.g.
    /// to mark it in menus.
    pub fn is_scoped(&self, id: &str) -> bool {
        id != crate::builtin::settings::SCOPE_ONLY.as_str() && self.scoped_value(id).is_some()
    }

    /// Whether a change of `id` goes to the active scope.
    pub fn goes_to_scope(&self, id: &str) -> bool {
        let Some(scope) = &self.scope else {
            return false;
        };
        let only = crate::builtin::settings::SCOPE_ONLY;
        let scoped = self.specs.get(id).is_some_and(|spec| spec.scoped);
        scoped
            && (id == only.as_str() || scope.get(only.as_str()) == Some(&SettingValue::Bool(true)))
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

    #[test]
    fn scopes_override_scoped_settings_while_switched_on() {
        use crate::builtin::settings::SCOPE_ONLY;
        let mut s = Settings::new();
        let int = |id: &'static str, scoped: bool| {
            let spec = SettingSpec::new(
                Id::new_static(id),
                id,
                SettingKind::Int { min: 0, max: 99 },
                SettingValue::Int(10),
            );
            if scoped { spec.scoped() } else { spec }
        };
        s.define(int("a.size", true)).unwrap();
        s.define(int("a.other", false)).unwrap();
        s.define(
            SettingSpec::new(
                SCOPE_ONLY,
                "x",
                SettingKind::Bool,
                SettingValue::Bool(false),
            )
            .scoped(),
        )
        .unwrap();
        s.set("a.size", SettingValue::Int(12)).unwrap();
        assert!(!s.goes_to_scope("a.size"));

        // A book with its own size, switched on.
        let mut values = VecMap::new();
        values.insert(Id::new_static("a.size"), SettingValue::Int(20));
        values.insert(Id::new_static("a.other"), SettingValue::Int(30));
        values.insert(SCOPE_ONLY, SettingValue::Bool(true));
        s.set_scope(Some(values));
        assert_eq!(s.int("a.size"), 20);
        assert!(s.is_scoped("a.size"));
        // Not scoped: the book cannot change it.
        assert_eq!(s.int("a.other"), 10);
        assert!(s.goes_to_scope("a.size") && !s.goes_to_scope("a.other"));
        s.set_in_scope("a.size", SettingValue::Int(25)).unwrap();
        assert_eq!(
            (s.int("a.size"), s.global("a.size")),
            (25, Some(&SettingValue::Int(12)))
        );

        // Switched off: the general value again, the book's dropped.
        s.set_in_scope(SCOPE_ONLY.as_str(), SettingValue::Bool(false))
            .unwrap();
        assert_eq!(s.int("a.size"), 12);
        assert!(s.scope().unwrap().is_empty());
        // Only the switch goes to the scope now.
        assert!(!s.goes_to_scope("a.size") && s.goes_to_scope(SCOPE_ONLY.as_str()));
        s.set_scope(None);
        assert_eq!(s.int("a.size"), 12);
        assert!(!s.goes_to_scope(SCOPE_ONLY.as_str()));
    }

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
