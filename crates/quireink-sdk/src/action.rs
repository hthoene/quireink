use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::collections::VecMap;
use crate::command::Command;
use crate::env::Env;
use crate::id::Id;

/// Global handler of an action, called when the active screen does not
/// handle it. It reacts by emitting commands.
pub type ActionHandler = Box<dyn Fn(&Env<'_>, &mut Vec<Command>)>;

/// A command users can bind to buttons, taps and gestures.
///
/// Actions without a handler are *screen actions*: their meaning depends on
/// the active screen (e.g. `core.next-page` in a reader vs. in a list).
pub struct ActionSpec {
    pub id: Id,
    /// Translation key of the action's display name (for key map editors).
    pub title: Cow<'static, str>,
    pub handler: Option<ActionHandler>,
}

impl fmt::Debug for ActionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("handler", &self.handler.is_some())
            .finish()
    }
}

/// Every registered action, e.g. for menu and key map editors.
#[derive(Debug, Default)]
pub struct Actions {
    map: VecMap<Id, ActionSpec>,
}

impl Actions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, action: ActionSpec) {
        self.map.insert(action.id.clone(), action);
    }

    pub fn get(&self, id: &str) -> Option<&ActionSpec> {
        self.map.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.map.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActionSpec> {
        self.map.values()
    }

    /// Releases unused capacity once registration is complete.
    pub fn shrink_to_fit(&mut self) {
        self.map.shrink_to_fit();
    }
}
