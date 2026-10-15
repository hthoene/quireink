//! What the key map binds, as plugins see it, e.g. to show and change
//! the bindings ([`Command::Bind`](crate::Command::Bind)).
//!
//! Inputs are named as in `quireink.ini`: `button.<name>`, `long.<name>`
//! (held), `tap.<zone>`, `long-tap.<zone>` (zones `top-left` … `bottom-right`)
//! and `swipe.<left|right|up|down>`. Layers are screen or slot ids, and
//! [`GLOBAL_KEYMAP_LAYER`](crate::builtin::GLOBAL_KEYMAP_LAYER).

use crate::id::Id;

/// The bindings in effect.
pub trait KeyBindings {
    /// The action `input` runs, looked up in `layers` from the most
    /// specific (a screen, its slot) to `global`, as input is; `None` if
    /// it runs none.
    fn bound(&self, layers: &[&str], input: &str) -> Option<Id>;
}
