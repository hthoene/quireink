//! OPDS catalogs: online libraries such as Project Gutenberg, Calibre-Web,
//! Kavita or Komga, browsed and searched like folders; their books are
//! downloaded into the library.
//!
//! - *Catalogs* lists them. Tapping one browses it; holding it (with
//!   buttons: the menu button) offers to sync, edit or remove it.
//! - A catalog has an address and, if it needs them, a user name and a
//!   password. Marked for syncing, *Sync catalogs* downloads its books
//!   that are not on the device yet into its folder (see [`sync`]); the
//!   action can also be put into menus.
//! - Books go to the library folder when downloaded one by one, in the
//!   format QuireInk reads best of those offered.
//!
//! Feeds are read in pieces and books written straight to storage, so
//! catalogs of any size work on small devices too.

#![no_std]

extern crate alloc;

mod browse;
mod catalogs;
mod edit;
mod feed;
mod list;
mod sync;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{groups, menus, params};
use quireink_sdk::ui::choice::Choice;
use quireink_sdk::{
    ActionSpec, Command, Env, HttpRequest, Id, Manifest, MenuEntry, Params, Plugin, RegisterError,
    Registrar, SDK_VERSION, ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

use crate::catalogs::Catalog;
use crate::sync::Sync;

pub const PLUGIN: Id = Id::new_static("opds");
/// Opens the list of catalogs.
pub const OPEN: Id = Id::new_static("opds.open");
/// Syncs the catalogs marked for it.
pub const SYNC: Id = Id::new_static("opds.sync");
const LIST_SCREEN: Id = Id::new_static("opds.catalogs");
const BROWSE_SCREEN: Id = Id::new_static("opds.browse");
const EDIT_SCREEN: Id = Id::new_static("opds.edit");
const CHOICE: Id = Id::new_static("opds.choice");
/// Translation keys of the choices: this, then the option.
const OPTION_LABELS: &str = "opds.option.";

/// Parameters: an address, and which catalog (its place in the list).
const URL: &str = params::URL;
const ID: &str = params::ID;

const USER_AGENT: &str = "QuireInk/0.1 (e-reader)";
/// Setting: covers in the lists of catalogs (on devices with the memory).
const COVERS: Id = Id::new_static("opds.covers");

/// A request to a catalog, with its credentials.
fn request(
    catalog: &Catalog,
    url: &str,
    save_to: String,
    reply: Id,
    params: Params,
) -> HttpRequest {
    let mut request = HttpRequest {
        save_to: Some(save_to),
        params,
        ..HttpRequest::new("GET", url, reply)
    };
    request
        .headers
        .push((String::from("User-Agent"), String::from(USER_AGENT)));
    if let Some(auth) = catalog.authorization() {
        request.headers.push((String::from("Authorization"), auth));
    }
    request
}

/// A request for a list of a catalog: for a WebDAV folder its listing
/// (`PROPFIND`), else the feed.
fn list_request(
    catalog: &Catalog,
    url: &str,
    save_to: String,
    reply: Id,
    params: Params,
) -> HttpRequest {
    let mut request = request(catalog, url, save_to, reply, params);
    if catalog.webdav {
        request.method = String::from("PROPFIND");
        request
            .headers
            .push((String::from("Depth"), String::from("1")));
        request.headers.push((
            String::from("Content-Type"),
            String::from("application/xml; charset=utf-8"),
        ));
        request.body = Some(String::from(feed::PROPFIND));
    }
    request
}

/// Reads a list of a catalog from `path`, which came from `base`.
fn read_list(
    catalog: &Catalog,
    storage: &dyn quireink_sdk::Storage,
    path: &str,
    base: &str,
) -> feed::Feed {
    if catalog.webdav {
        feed::read_webdav(storage, path, base)
    } else {
        feed::read(storage, path, base)
    }
}

/// The catalogs marked for syncing.
fn marked(env: &Env<'_>) -> Vec<Catalog> {
    catalogs::load(env.storage)
        .into_iter()
        .filter(|c| c.sync)
        .collect()
}

/// The OPDS plugin.
#[derive(Debug, Default)]
pub struct Opds;

impl Plugin for Opds {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("OPDS catalogs"),
            description: Cow::Borrowed(
                "Browses OPDS catalogs, downloads their books and keeps folders in sync with them.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let state = Rc::new(RefCell::new(Sync::default()));
        let shared = state.clone();
        r.add_screen(ScreenSpec {
            id: LIST_SCREEN,
            title: Cow::Borrowed("opds.title"),
            factory: Box::new(move |env, _| Box::new(list::ListScreen::new(env, shared.clone()))),
        })?;
        r.add_screen(ScreenSpec {
            id: BROWSE_SCREEN,
            title: Cow::Borrowed("opds.title"),
            factory: Box::new(|env, params| Box::new(browse::BrowseScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: EDIT_SCREEN,
            title: Cow::Borrowed("opds.title"),
            factory: Box::new(|env, params| Box::new(edit::EditScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("opds.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("opds.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(LIST_SCREEN, Params::new()));
            })),
        })?;
        let shared = state.clone();
        r.add_action(ActionSpec {
            id: SYNC,
            title: Cow::Borrowed("opds.sync-all"),
            handler: Some(Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                shared.borrow_mut().start(env, marked(env), out);
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 45,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("cloud-download"))?;
        r.add_icon(SYNC, Cow::Borrowed("refresh"))?;
        for (event, book) in [(sync::FEED_LOADED, false), (sync::BOOK_LOADED, true)] {
            let shared = state.clone();
            r.subscribe(
                event,
                Box::new(
                    move |env: &Env<'_>, params: &Params, out: &mut Vec<Command>| {
                        let mut sync = shared.borrow_mut();
                        if book {
                            sync.book_loaded(env, params, out);
                        } else {
                            sync.feed_loaded(env, params, out);
                        }
                    },
                ),
            )?;
        }
        r.add_setting(
            SettingSpec::new(
                COVERS,
                "opds.setting.covers",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .group(groups::LIBRARY)
            .section("opds.section")
            .description("opds.setting.covers.help"),
        )?;
        r.add_setting(
            SettingSpec::new(
                sync::LIMIT,
                "opds.setting.limit",
                SettingKind::Int { min: 10, max: 1000 },
                SettingValue::Int(50),
            )
            .group(groups::LIBRARY)
            .section("opds.section")
            .step(10)
            .description("opds.setting.limit.help"),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}
