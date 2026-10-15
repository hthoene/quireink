//! Searching the settings: what was typed, found in the titles and
//! descriptions of every setting shown here and of the pages other
//! plugins add; choosing one opens its page with it focused.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus, params, slots};
use quireink_sdk::ui::{self, ListView, UiStyle};
use quireink_sdk::{Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Screen};

use crate::{FOCUS, GROUP_SCREEN, ID, moved, navigate, page, visible};

/// The results; parameter [`params::TEXT`], what to find.
pub(crate) const SCREEN: Id = Id::new_static("settings.search");
/// Emitted by the keyboard with what to find.
pub(crate) const ENTERED: Id = Id::new_static("settings.search-entered");

/// Asks for what to find.
pub(crate) fn ask(env: &Env<'_>) -> Command {
    Command::OpenSlot(
        slots::KEYBOARD,
        Params::new()
            .with(params::TITLE, env.i18n.tr("settings.search").to_string())
            .with(params::REPLY, ENTERED.as_str()),
    )
}

/// Where a result leads.
#[derive(Debug, Clone)]
enum Target {
    /// A setting on its group's page.
    Setting { group: Id, id: Id },
    /// A page of another plugin: its action.
    Page(Id),
}

#[derive(Debug, Clone)]
struct Hit {
    title: String,
    /// Where it is, e.g. its group.
    note: String,
    target: Target,
}

/// `text` in lower case without accents, so "rander" finds "Ränder".
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let plain = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
            'ç' | 'č' | 'ć' => "c",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ě' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' | 'ń' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ő' => "o",
            'ù' | 'ú' | 'û' | 'ü' | 'ů' | 'ű' => "u",
            'ý' | 'ÿ' => "y",
            'š' | 'ś' => "s",
            'ž' | 'ź' | 'ż' => "z",
            'ß' => "ss",
            'æ' => "ae",
            'œ' => "oe",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(plain);
    }
    out
}

/// Whether every word of `query` is in `text`, both folded (see [`fold`]).
fn matches(text: &str, words: &[String]) -> bool {
    let text = fold(text);
    words.iter().all(|w| text.contains(w.as_str()))
}

fn find(env: &Env<'_>, query: &str) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(fold).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for spec in env.settings.specs().filter(|spec| visible(env, spec)) {
        let title = env.i18n.tr(&spec.title);
        let description = spec.description.as_deref().map_or("", |d| env.i18n.tr(d));
        if !matches(&alloc::format!("{title} {description}"), &words) {
            continue;
        }
        let group = env.settings.group_of(spec).clone();
        let note = env
            .settings
            .groups()
            .find(|g| g.id == group)
            .map_or_else(String::new, |g| env.i18n.tr(&g.title).to_string());
        hits.push(Hit {
            title: title.to_string(),
            note,
            target: Target::Setting {
                group,
                id: spec.id.clone(),
            },
        });
    }
    let settings_title = env.i18n.tr("settings.title");
    for item in env.menus.items(menus::SETTINGS.as_str()) {
        let title = env.i18n.tr(item.title);
        if !item.section && matches(title, &words) {
            hits.push(Hit {
                title: title.to_string(),
                note: settings_title.to_string(),
                target: Target::Page(item.id.clone()),
            });
        }
    }
    hits
}

/// The results.
pub(crate) struct SearchScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    title: String,
    hits: Vec<Hit>,
}

impl SearchScreen {
    pub(crate) fn new(env: &Env<'_>, p: &Params) -> Self {
        let (style, header, area) = page(env);
        let query = p.get(params::TEXT).unwrap_or_default().trim();
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            title: env.i18n.format(
                "settings.search.title",
                &[("query", quireink_sdk::Arg::Str(query))],
            ),
            hits: find(env, query),
            style,
            header,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        match self.hits.get(index).map(|h| &h.target) {
            Some(Target::Setting { group, id }) => cx.command(Command::OpenScreen(
                GROUP_SCREEN,
                Params::new()
                    .with(ID, group.as_str().to_string())
                    .with(FOCUS, id.as_str().to_string()),
            )),
            Some(Target::Page(action)) => cx.command(Command::RunAction(action.clone())),
            None => {}
        }
    }
}

impl Screen for SearchScreen {
    fn draw(&mut self, canvas: &mut quireink_sdk::Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.hits.len();
        self.header.draw(
            canvas,
            env,
            &style,
            &self.title,
            None,
            self.list.paging(len),
        );
        if len == 0 {
            let none = env.i18n.tr("settings.search.none");
            ui::draw_message(canvas, env, &style, self.list.area, none);
            return;
        }
        let hits = &self.hits;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let hit = &hits[index];
                ui::draw_noted_row(
                    canvas, env, &style, rect, focused, &hit.title, "›", &hit.note,
                );
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if let Some(change) = navigate(&mut self.list, self.hits.len(), action) {
            moved(change, cx);
            return Handled::Yes;
        }
        if *action == actions::CONFIRM {
            self.choose(self.list.focus, cx);
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let paging = self.list.paging(self.hits.len());
        let tapped = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging);
        if let Some(action) = tapped {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.hits.len()) {
            Some(index) => {
                self.list.focus = index;
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_counts() {
        let words = |q: &str| q.split_whitespace().map(fold).collect::<Vec<_>>();
        assert!(matches("Font size of the book's text", &words("SIZE font")));
        assert!(!matches("Font size", &words("font weight")));
        assert!(matches("Schriftgröße", &words("größe")));
        // Without the accents, either way.
        assert!(matches("Ränder", &words("rand")));
        assert!(matches("Schriftgrösse", &words("grosse")));
        assert!(matches("Café", &words("cafe")));
    }
}
