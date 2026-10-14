//! The plugins the kernel knows about: running, switched off or failed.
//!
//! Plugin managers read this list through [`Env::plugins`](crate::Env) and
//! switch plugins with [`Command::SetPluginEnabled`](crate::Command). The
//! change is saved to `quireink.ini` and applies after a restart
//! ([`Command::Restart`](crate::Command)).

use alloc::format;
use alloc::vec::Vec;

use quireink_i18n::Translator;

use crate::builtin::{fonts, slots};
use crate::id::Id;
use crate::plugin::Manifest;

/// What [`RequiredFor`] can name: the standard slots and the UI font.
static ESSENTIALS: [Id; 6] = [
    slots::HOME,
    slots::READER,
    slots::LIBRARY,
    slots::MENU,
    slots::SETTINGS,
    fonts::UI,
];

/// The standard slots (and `core.font-ui` for "all fonts") that only one
/// plugin provides. A bit set: no heap memory per plugin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequiredFor(u8);

impl RequiredFor {
    /// Adds `id`; `false` if it is not a standard slot or the UI font.
    pub fn insert(&mut self, id: &Id) -> bool {
        match ESSENTIALS.iter().position(|e| e == id) {
            Some(bit) => {
                self.0 |= 1 << bit;
                true
            }
            None => false,
        }
    }

    pub fn contains(&self, id: &Id) -> bool {
        ESSENTIALS
            .iter()
            .position(|e| e == id)
            .is_some_and(|bit| self.0 & (1 << bit) != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static Id> + '_ {
        ESSENTIALS
            .iter()
            .enumerate()
            .filter(|(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, id)| id)
    }
}

/// Whether a plugin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    /// Loaded and running.
    Running,
    /// Switched off in the configuration; not loaded.
    Off,
    /// Could not be loaded (see the kernel's warnings).
    Failed,
}

/// A plugin and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub manifest: Manifest,
    pub state: PluginState,
    /// Switched on in the configuration. Differs from `state` until the
    /// next start when the user has just switched the plugin.
    pub enabled: bool,
    /// Core slots no other plugin fills (e.g. `core.settings`), or
    /// `core.font-ui` if the plugin provides every font. Switching the
    /// plugin off would leave them empty, so it stays on until another
    /// plugin can take over.
    pub required_for: RequiredFor,
}

impl PluginInfo {
    /// The name in the current language: the plugin's translation of
    /// `<id>.plugin.name`, or the manifest's name.
    pub fn name<'a>(&'a self, i18n: &'a Translator) -> &'a str {
        i18n.get(&format!("{}.plugin.name", self.manifest.id))
            .unwrap_or(&self.manifest.name)
    }

    /// The description in the current language: the plugin's translation
    /// of `<id>.plugin.description`, or the manifest's description.
    pub fn description<'a>(&'a self, i18n: &'a Translator) -> &'a str {
        i18n.get(&format!("{}.plugin.description", self.manifest.id))
            .unwrap_or(&self.manifest.description)
    }

    /// Whether the user may switch the plugin off.
    pub fn can_switch_off(&self) -> bool {
        self.manifest.id != crate::builtin::CORE_PLUGIN && self.required_for.is_empty()
    }

    /// Whether a restart is needed to apply the user's choice.
    pub fn restart_needed(&self) -> bool {
        match self.state {
            PluginState::Running => !self.enabled,
            PluginState::Off => self.enabled,
            PluginState::Failed => false,
        }
    }
}

/// Every plugin given to the kernel, in registration order.
#[derive(Debug, Default)]
pub struct Plugins {
    list: Vec<PluginInfo>,
}

impl Plugins {
    pub const fn new() -> Self {
        Self { list: Vec::new() }
    }

    pub fn iter(&self) -> impl Iterator<Item = &PluginInfo> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&PluginInfo> {
        self.list.iter().find(|p| p.manifest.id.as_str() == id)
    }

    /// Whether a plugin was switched since the start.
    pub fn restart_needed(&self) -> bool {
        self.list.iter().any(PluginInfo::restart_needed)
    }

    /// Adds a plugin (kernel only).
    pub fn push(&mut self, info: PluginInfo) {
        self.list.push(info);
    }

    /// Mutable access for the kernel.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut PluginInfo> {
        self.list.iter_mut().find(|p| p.manifest.id.as_str() == id)
    }

    pub fn shrink_to_fit(&mut self) {
        self.list.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::Cow;
    use alloc::vec::Vec;

    use super::*;
    use crate::{SDK_VERSION, Version};

    fn info(id: &'static str, state: PluginState, enabled: bool) -> PluginInfo {
        PluginInfo {
            manifest: Manifest {
                id: Id::new_static(id),
                name: Cow::Borrowed("Name"),
                description: Cow::Borrowed("Does things."),
                version: Version::new(1, 0, 0),
                sdk: SDK_VERSION,
            },
            state,
            enabled,
            required_for: RequiredFor::default(),
        }
    }

    #[test]
    fn restart_is_needed_only_for_switched_plugins() {
        let mut plugins = Plugins::new();
        plugins.push(info("a", PluginState::Running, true));
        plugins.push(info("b", PluginState::Failed, false));
        assert!(!plugins.restart_needed());
        plugins.get_mut("a").unwrap().enabled = false;
        assert!(plugins.restart_needed());
    }

    #[test]
    fn names_fall_back_to_the_manifest() {
        let mut i18n = Translator::new();
        let catalog = quireink_i18n::Catalog::parse(Cow::Borrowed(
            "[meta]\nlanguage = de\nname = Deutsch\nplural = one-other\n\n[messages]\na.plugin.name = Ah\n",
        ))
        .unwrap();
        i18n.add(catalog);
        i18n.set_language("de");
        let a = info("a", PluginState::Running, true);
        assert_eq!(a.name(&i18n), "Ah");
        assert_eq!(a.description(&i18n), "Does things.");
        let core = info("core", PluginState::Running, true);
        assert!(!core.can_switch_off());
        assert!(a.can_switch_off());
    }

    #[test]
    fn required_for_holds_standard_slots_only() {
        let mut needs = RequiredFor::default();
        assert!(needs.insert(&slots::MENU));
        assert!(needs.insert(&fonts::UI));
        assert!(!needs.insert(&Id::new_static("x.slot")));
        assert!(needs.contains(&slots::MENU));
        assert!(!needs.contains(&slots::HOME));
        let ids: Vec<&Id> = needs.iter().collect();
        assert_eq!(ids, [&slots::MENU, &fonts::UI]);
    }
}
