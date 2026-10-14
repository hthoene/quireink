//! Layered key maps: raw input → action id.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_platform::Point;
use quireink_sdk::builtin::GLOBAL_KEYMAP_LAYER;
use quireink_sdk::collections::VecMap;
use quireink_sdk::{Direction, Id};

/// Section prefix of key map layers in config files.
const SECTION_PREFIX: &str = "keymap.";
/// Value that removes a binding.
const UNBOUND: &str = "none";

/// One of nine tap zones (a 3×3 grid over the screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Zone {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Zone {
    const ALL: [Zone; 9] = [
        Zone::TopLeft,
        Zone::Top,
        Zone::TopRight,
        Zone::Left,
        Zone::Center,
        Zone::Right,
        Zone::BottomLeft,
        Zone::Bottom,
        Zone::BottomRight,
    ];

    fn id(self) -> &'static str {
        match self {
            Zone::TopLeft => "top-left",
            Zone::Top => "top",
            Zone::TopRight => "top-right",
            Zone::Left => "left",
            Zone::Center => "center",
            Zone::Right => "right",
            Zone::BottomLeft => "bottom-left",
            Zone::Bottom => "bottom",
            Zone::BottomRight => "bottom-right",
        }
    }

    fn from_id(id: &str) -> Option<Zone> {
        Zone::ALL.into_iter().find(|zone| zone.id() == id)
    }

    /// The zone containing `point` on a `width`×`height` screen.
    pub(crate) fn of(point: Point, width: u16, height: u16) -> Zone {
        let third = |value: i32, size: u16| {
            let size = i64::from(size.max(1));
            (i64::from(value).clamp(0, size - 1) * 3 / size) as usize
        };
        Zone::ALL[third(point.y, height) * 3 + third(point.x, width)]
    }
}

/// An input as written in key maps.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum InputKey {
    /// `button.<name>`
    Button(Cow<'static, str>),
    /// `long.<name>`
    LongButton(Cow<'static, str>),
    /// `tap.<zone>`
    Tap(Zone),
    /// `long-tap.<zone>`
    LongTap(Zone),
    /// `swipe.<direction>`
    Swipe(Direction),
}

impl InputKey {
    fn parse(key: &str) -> Option<InputKey> {
        let (kind, name) = key.split_once('.')?;
        let button = |name: &str| {
            let valid = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            valid.then(|| Cow::Owned(name.to_string()))
        };
        match kind {
            "button" => button(name).map(InputKey::Button),
            "long" => button(name).map(InputKey::LongButton),
            "tap" => Zone::from_id(name).map(InputKey::Tap),
            "long-tap" => Zone::from_id(name).map(InputKey::LongTap),
            "swipe" => [
                Direction::Left,
                Direction::Right,
                Direction::Up,
                Direction::Down,
            ]
            .into_iter()
            .find(|d| d.id() == name)
            .map(InputKey::Swipe),
            _ => None,
        }
    }
}

/// Where a binding comes from. Later sources override earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Origin {
    Plugin,
    Device,
    /// The preset the user selected.
    Preset,
    User,
}

#[derive(Debug, Clone)]
struct Binding {
    /// `None` means explicitly unbound.
    action: Option<Id>,
    origin: Origin,
}

/// A problem in a key map source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub line: u32,
    pub message: Cow<'static, str>,
}

#[derive(Debug, Default)]
pub(crate) struct Keymap {
    layers: VecMap<String, VecMap<InputKey, Binding>>,
}

impl Keymap {
    /// Releases unused capacity once all sources are loaded.
    pub(crate) fn shrink_to_fit(&mut self) {
        self.layers.shrink_to_fit();
        for layer in self.layers.values_mut() {
            layer.shrink_to_fit();
        }
    }

    /// Loads bindings from `source`.
    ///
    /// With `strict`, sections other than `[keymap.*]` are problems;
    /// otherwise (user configuration files) they are skipped.
    pub(crate) fn load(&mut self, source: &str, origin: Origin, strict: bool) -> Vec<Problem> {
        let mut problems = Vec::new();
        let problem = |problems: &mut Vec<Problem>, line, message: &'static str| {
            problems.push(Problem {
                line,
                message: Cow::Borrowed(message),
            });
        };
        for entry in quireink_config::parse(source) {
            let entry = match entry {
                Ok(entry) => entry,
                // Non-strict sources are whole config files whose syntax
                // errors are reported by their main reader.
                Err(_) if !strict => continue,
                Err(error) => {
                    problems.push(Problem {
                        line: error.line,
                        message: Cow::Owned(error.to_string()),
                    });
                    continue;
                }
            };
            let Some(layer) = entry.section.strip_prefix(SECTION_PREFIX) else {
                if strict {
                    problem(
                        &mut problems,
                        entry.line,
                        "expected a [keymap.<layer>] section",
                    );
                }
                continue;
            };
            if layer != GLOBAL_KEYMAP_LAYER && Id::parse(layer).is_err() {
                problem(&mut problems, entry.line, "invalid key map layer");
                continue;
            }
            let Some(input) = InputKey::parse(entry.key) else {
                problem(
                    &mut problems,
                    entry.line,
                    "expected button.<name>, long.<name>, tap.<zone>, long-tap.<zone> or swipe.<dir>",
                );
                continue;
            };
            let value = entry.value.as_str();
            let action = if value == UNBOUND {
                None
            } else if let Ok(id) = Id::parse(&value) {
                Some(id)
            } else {
                problem(&mut problems, entry.line, "invalid action id");
                continue;
            };
            let bindings = self
                .layers
                .get_or_insert_with(String::from(layer), VecMap::new);
            let overrides = bindings.get(&input).is_none_or(|b| origin >= b.origin);
            if overrides {
                bindings.insert(input, Binding { action, origin });
            }
        }
        problems
    }

    /// Resolves `input` through `layers`, most specific first. An explicit
    /// `none` binding stops the search.
    pub(crate) fn resolve(&self, layers: &[&str], input: &InputKey) -> Option<&Id> {
        layers
            .iter()
            .find_map(|layer| self.layers.get(*layer)?.get(input))
            .and_then(|binding| binding.action.as_ref())
    }

    /// All bound action ids, for validation.
    pub(crate) fn actions(&self) -> impl Iterator<Item = &Id> {
        self.layers
            .values()
            .flat_map(|layer| layer.values())
            .filter_map(|binding| binding.action.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(name: &'static str) -> InputKey {
        InputKey::Button(Cow::Borrowed(name))
    }

    #[test]
    fn zones_split_the_screen_in_thirds() {
        assert_eq!(Zone::of(Point::new(0, 0), 300, 300), Zone::TopLeft);
        assert_eq!(Zone::of(Point::new(150, 150), 300, 300), Zone::Center);
        assert_eq!(Zone::of(Point::new(299, 100), 300, 300), Zone::Right);
        assert_eq!(Zone::of(Point::new(500, 500), 300, 300), Zone::BottomRight);
    }

    #[test]
    fn layers_and_origins() {
        let mut keymap = Keymap::default();
        let problems = keymap.load(
            "[keymap.global]\nbutton.confirm = core.confirm\ntap.center = core.menu\n\
             [keymap.reader.screen]\nbutton.confirm = reader.bookmark\n",
            Origin::Plugin,
            true,
        );
        assert!(problems.is_empty(), "{problems:?}");

        let global = ["global"];
        let reader = ["reader.screen", "global"];
        let confirm = button("confirm");
        assert_eq!(
            keymap.resolve(&global, &confirm).map(Id::as_str),
            Some("core.confirm")
        );
        assert_eq!(
            keymap.resolve(&reader, &confirm).map(Id::as_str),
            Some("reader.bookmark")
        );
        assert_eq!(
            keymap
                .resolve(&reader, &InputKey::Tap(Zone::Center))
                .map(Id::as_str),
            Some("core.menu")
        );

        // A plugin cannot override the user, but the user overrides everything.
        keymap.load(
            "[keymap.global]\nbutton.confirm = user.thing\n",
            Origin::User,
            false,
        );
        keymap.load(
            "[keymap.global]\nbutton.confirm = other.thing\n",
            Origin::Plugin,
            true,
        );
        assert_eq!(
            keymap.resolve(&global, &confirm).map(Id::as_str),
            Some("user.thing")
        );
    }

    #[test]
    fn none_unbinds() {
        let mut keymap = Keymap::default();
        keymap.load(
            "[keymap.global]\ntap.center = core.menu\n",
            Origin::Plugin,
            true,
        );
        keymap.load("[keymap.global]\ntap.center = none\n", Origin::User, false);
        assert_eq!(
            keymap.resolve(&["global"], &InputKey::Tap(Zone::Center)),
            None
        );
    }

    #[test]
    fn reports_problems() {
        let mut keymap = Keymap::default();
        let problems = keymap.load(
            "[other]\na = b\n[keymap.global]\nbutton.X = core.menu\ntap.middle = core.menu\n\
             button.ok = Not An Id\n[keymap.Bad Layer]\n",
            Origin::Plugin,
            true,
        );
        let lines: Vec<u32> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [2, 4, 5, 6, 7]);
    }
}
