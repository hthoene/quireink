//! What a screen shows, as parts with roles and text (PLANNING G11): for
//! reading the interface aloud, automation and tests.
//!
//! Nothing has to be written for it: the drawing helpers of [`ui`](crate::ui)
//! tell what they draw ([`Canvas::describe`](crate::Canvas::describe)), list
//! rows with their focus, the text in them, buttons, switches and
//! headers. [`parts`] puts that together: the text drawn inside a row
//! becomes the row's. Screens that draw in their own way describe their
//! parts themselves.

use alloc::string::String;
use alloc::vec::Vec;

pub use quireink_graphics::{Described, Role};
use quireink_platform::Rect;

/// A part of a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub role: Role,
    /// Its text: its own, or that drawn inside it.
    pub label: String,
    pub area: Rect,
    pub focused: bool,
    /// Switched on or off, for switches (also a row's) and buttons that
    /// are either.
    pub on: Option<bool>,
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// The parts of what was drawn, in drawing order: rows, headings and
/// buttons with the text and switches drawn inside them.
pub fn parts(described: Vec<Described>) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    // Whether a part's text is its own (the text drawn inside it is then
    // the same, not added).
    let mut own: Vec<bool> = Vec::new();
    // Text inside parts: the part, where, what.
    let mut texts: Vec<(usize, Rect, String)> = Vec::new();
    for d in described {
        // Inside the last part drawn around it.
        let container = parts.iter().rposition(|p| {
            matches!(p.role, Role::Row | Role::Heading | Role::Button) && inside(d.area, p.area)
        });
        // Arrows and marks say nothing.
        if d.role == Role::Text && !d.label.chars().any(char::is_alphanumeric) {
            continue;
        }
        match (d.role, container) {
            (Role::Text, Some(i)) => {
                if !own[i] && !d.label.trim().is_empty() {
                    texts.push((i, d.area, d.label));
                }
            }
            (Role::Switch, Some(i)) if parts[i].role == Role::Row => parts[i].on = d.on,
            _ => {
                own.push(!d.label.is_empty());
                parts.push(Part {
                    role: d.role,
                    label: d.label,
                    area: d.area,
                    focused: d.focused,
                    on: d.on,
                });
            }
        }
    }
    // In reading order: by line, then from the left.
    texts.sort_by_key(|(i, area, _)| (*i, area.y, area.x));
    for (i, _, text) in texts {
        let part = &mut parts[i];
        if !part.label.is_empty() {
            part.label.push_str(", ");
        }
        // Without the arrows that lead on.
        part.label.push_str(text.trim_matches(['›', '‹', ' ']));
    }
    parts.retain(|p| !(p.role == Role::Text && p.label.trim().is_empty()));
    parts
}

/// The part with the focus, if any.
pub fn focused(parts: &[Part]) -> Option<&Part> {
    parts.iter().rev().find(|p| p.focused)
}

/// The first part whose text is `label` (ignoring case), else the first
/// one whose text begins with it.
pub fn find<'a>(parts: &'a [Part], label: &str) -> Option<&'a Part> {
    let label = label.to_lowercase();
    parts
        .iter()
        .find(|p| p.label.to_lowercase() == label)
        .or_else(|| {
            parts
                .iter()
                .find(|p| p.label.to_lowercase().starts_with(&label))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(role: Role, label: &str, area: Rect, focused: bool, on: Option<bool>) -> Described {
        Described {
            role,
            label: String::from(label),
            area,
            focused,
            on,
        }
    }

    #[test]
    fn rows_take_the_text_inside() {
        let row = Rect::new(0, 100, 400, 60);
        let parts = parts(alloc::vec![
            d(
                Role::Heading,
                "Settings",
                Rect::new(0, 0, 400, 50),
                false,
                None
            ),
            d(
                Role::Text,
                "Settings",
                Rect::new(10, 10, 200, 30),
                false,
                None
            ),
            d(Role::Row, "", row, true, None),
            d(
                Role::Text,
                "Night mode",
                Rect::new(10, 105, 200, 30),
                false,
                None
            ),
            d(
                Role::Switch,
                "",
                Rect::new(340, 110, 50, 30),
                false,
                Some(true)
            ),
            d(
                Role::Text,
                "Inverts the screen",
                Rect::new(10, 135, 300, 20),
                false,
                None
            ),
            d(Role::Button, "OK", Rect::new(0, 300, 100, 50), false, None),
            d(Role::Text, "OK", Rect::new(10, 310, 50, 30), false, None),
            d(Role::Text, "Alone", Rect::new(0, 400, 50, 30), false, None),
        ]);
        let labels: Vec<(&str, Role)> = parts.iter().map(|p| (p.label.as_str(), p.role)).collect();
        assert_eq!(
            labels,
            [
                ("Settings", Role::Heading),
                ("Night mode, Inverts the screen", Role::Row),
                ("OK", Role::Button),
                ("Alone", Role::Text),
            ]
        );
        assert_eq!(parts[1].on, Some(true));
        assert_eq!(
            focused(&parts).map(|p| p.label.as_str()),
            Some("Night mode, Inverts the screen")
        );
        assert_eq!(find(&parts, "night").map(|p| p.area), Some(row));
        assert_eq!(find(&parts, "ok").map(|p| p.role), Some(Role::Button));
    }
}
