//! Contribution registries and plugin registration.

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec::Vec;

use quireink_i18n::Catalog;
use quireink_sdk::{
    ActionSpec, EventHandler, FontSpec, FormatSpec, Id, MenuEntry, Priority, RegisterError,
    Registrar, ScreenSpec, SettingSpec, SlotSpec,
};

/// A candidate screen for a slot, proposed by a plugin.
#[derive(Debug)]
struct SlotCandidate {
    slot: Id,
    screen: Id,
    priority: Priority,
}

/// All active contributions except settings, key maps and translations,
/// which live in their own stores.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    pub(crate) actions: BTreeMap<Id, ActionSpec>,
    pub(crate) screens: BTreeMap<Id, ScreenSpec>,
    pub(crate) slots: BTreeMap<Id, SlotSpec>,
    candidates: Vec<SlotCandidate>,
    /// Slot bindings from the user's configuration.
    user_bindings: BTreeMap<Id, Id>,
}

impl Registry {
    /// The screen bound to `slot`: the user's choice if that screen exists,
    /// otherwise the candidate with the highest priority (the later
    /// registration wins a tie).
    pub(crate) fn resolve_slot(&self, slot: &str) -> Option<&Id> {
        if let Some(screen) = self.user_bindings.get(slot)
            && self.screens.contains_key(screen)
        {
            return Some(screen);
        }
        self.candidates
            .iter()
            .filter(|c| c.slot.as_str() == slot && self.screens.contains_key(&c.screen))
            .max_by_key(|c| c.priority)
            .map(|c| &c.screen)
    }

    pub(crate) fn bind_user(&mut self, slot: Id, screen: Id) {
        self.user_bindings.insert(slot, screen);
    }

    pub(crate) fn user_bindings(&self) -> impl Iterator<Item = (&Id, &Id)> {
        self.user_bindings.iter()
    }
}

/// Contributions of one plugin, collected during `register` and merged only
/// if the whole registration succeeds — a failing plugin leaves no traces.
#[derive(Default)]
pub(crate) struct Staging {
    actions: Vec<ActionSpec>,
    screens: Vec<ScreenSpec>,
    slots: Vec<SlotSpec>,
    candidates: Vec<SlotCandidate>,
    pub(crate) settings: Vec<SettingSpec>,
    pub(crate) keymaps: Vec<Cow<'static, str>>,
    pub(crate) catalogs: Vec<Catalog>,
    pub(crate) fonts: Vec<FontSpec>,
    pub(crate) font_roles: Vec<(Id, Id, Priority)>,
    pub(crate) formats: Vec<FormatSpec>,
    pub(crate) subscriptions: Vec<(Id, EventHandler)>,
    pub(crate) menu_entries: Vec<MenuEntry>,
}

impl Staging {
    /// Moves actions, screens, slots and bindings into `registry`.
    /// Settings, key maps and catalogs are left for the caller.
    pub(crate) fn merge_into(&mut self, registry: &mut Registry) -> Result<(), RegisterError> {
        let duplicate = self
            .actions
            .iter()
            .map(|a| &a.id)
            .find(|id| registry.actions.contains_key(*id))
            .or_else(|| {
                self.screens
                    .iter()
                    .map(|s| &s.id)
                    .find(|id| registry.screens.contains_key(*id))
            })
            .or_else(|| {
                self.slots
                    .iter()
                    .map(|s| &s.id)
                    .find(|id| registry.slots.contains_key(*id))
            });
        if let Some(id) = duplicate {
            return Err(RegisterError::Duplicate { id: id.clone() });
        }
        for action in self.actions.drain(..) {
            registry.actions.insert(action.id.clone(), action);
        }
        for screen in self.screens.drain(..) {
            registry.screens.insert(screen.id.clone(), screen);
        }
        for slot in self.slots.drain(..) {
            registry.slots.insert(slot.id.clone(), slot);
        }
        registry.candidates.append(&mut self.candidates);
        Ok(())
    }
}

/// The [`Registrar`] handed to a plugin.
pub(crate) struct PluginRegistrar<'a> {
    plugin: Id,
    staging: &'a mut Staging,
}

impl<'a> PluginRegistrar<'a> {
    pub(crate) fn new(plugin: Id, staging: &'a mut Staging) -> Self {
        Self { plugin, staging }
    }

    fn check_namespace(&self, id: &Id) -> Result<(), RegisterError> {
        if id.is_owned_by(&self.plugin) {
            Ok(())
        } else {
            Err(RegisterError::Namespace { id: id.clone() })
        }
    }

    fn check_unique<'i>(
        mut ids: impl Iterator<Item = &'i Id>,
        id: &Id,
    ) -> Result<(), RegisterError> {
        if ids.any(|existing| existing == id) {
            Err(RegisterError::Duplicate { id: id.clone() })
        } else {
            Ok(())
        }
    }
}

impl Registrar for PluginRegistrar<'_> {
    fn plugin(&self) -> &Id {
        &self.plugin
    }

    fn add_action(&mut self, action: ActionSpec) -> Result<(), RegisterError> {
        self.check_namespace(&action.id)?;
        Self::check_unique(self.staging.actions.iter().map(|a| &a.id), &action.id)?;
        self.staging.actions.push(action);
        Ok(())
    }

    fn add_screen(&mut self, screen: ScreenSpec) -> Result<(), RegisterError> {
        self.check_namespace(&screen.id)?;
        Self::check_unique(self.staging.screens.iter().map(|s| &s.id), &screen.id)?;
        self.staging.screens.push(screen);
        Ok(())
    }

    fn add_slot(&mut self, slot: SlotSpec) -> Result<(), RegisterError> {
        self.check_namespace(&slot.id)?;
        Self::check_unique(self.staging.slots.iter().map(|s| &s.id), &slot.id)?;
        self.staging.slots.push(slot);
        Ok(())
    }

    fn bind_slot(&mut self, slot: Id, screen: Id, priority: Priority) -> Result<(), RegisterError> {
        self.staging.candidates.push(SlotCandidate {
            slot,
            screen,
            priority,
        });
        Ok(())
    }

    fn add_setting(&mut self, setting: SettingSpec) -> Result<(), RegisterError> {
        self.check_namespace(&setting.id)?;
        Self::check_unique(self.staging.settings.iter().map(|s| &s.id), &setting.id)?;
        self.staging.settings.push(setting);
        Ok(())
    }

    fn add_font(&mut self, font: FontSpec) -> Result<(), RegisterError> {
        self.check_namespace(&font.id)?;
        Self::check_unique(self.staging.fonts.iter().map(|f| &f.id), &font.id)?;
        self.staging.fonts.push(font);
        Ok(())
    }

    fn bind_font(&mut self, role: Id, font: Id, priority: Priority) -> Result<(), RegisterError> {
        self.staging.font_roles.push((role, font, priority));
        Ok(())
    }

    fn add_format(&mut self, format: FormatSpec) -> Result<(), RegisterError> {
        self.check_namespace(&format.id)?;
        Self::check_unique(self.staging.formats.iter().map(|f| &f.id), &format.id)?;
        self.staging.formats.push(format);
        Ok(())
    }

    fn add_menu_entry(&mut self, entry: MenuEntry) -> Result<(), RegisterError> {
        self.staging.menu_entries.push(entry);
        Ok(())
    }

    fn subscribe(&mut self, event: Id, handler: EventHandler) -> Result<(), RegisterError> {
        self.staging.subscriptions.push((event, handler));
        Ok(())
    }

    fn add_keymap(&mut self, source: Cow<'static, str>) -> Result<(), RegisterError> {
        // Validate now so errors are attributed to the plugin; the kernel
        // loads the source again when merging.
        let mut scratch = crate::keymap::Keymap::default();
        let problems = scratch.load(&source, crate::keymap::Origin::Plugin, true);
        if let Some(problem) = problems.into_iter().next() {
            return Err(RegisterError::Parse {
                line: problem.line,
                message: problem.message,
            });
        }
        self.staging.keymaps.push(source);
        Ok(())
    }

    fn add_translations(&mut self, source: Cow<'static, str>) -> Result<(), RegisterError> {
        let catalog = Catalog::parse(&source).map_err(|error| RegisterError::Parse {
            line: match &error {
                quireink_i18n::CatalogError::Syntax(e) => e.line,
                quireink_i18n::CatalogError::UnknownPluralRule { line }
                | quireink_i18n::CatalogError::UnexpectedSection { line } => *line,
                _ => 0,
            },
            message: Cow::Owned(error.to_string()),
        })?;
        self.staging.catalogs.push(catalog);
        Ok(())
    }
}
