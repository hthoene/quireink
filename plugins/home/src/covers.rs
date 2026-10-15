//! Where the covers of recent books are, once looked up: a line each,
//! `path<TAB>width<TAB>height<TAB>picture`, with an empty picture for a
//! book without one. Looking a cover up opens the book (its format plugin
//! caches the picture), so the home screen does it a book at a time
//! while it shows, and its widgets only read this file.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::document::ImageRef;
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;

/// Books remembered.
const LIMIT: usize = 24;

fn file() -> String {
    format!("{}/covers.txt", Env::plugin_dir(PLUGIN.as_str()))
}

/// Every book looked up, with its cover if it has one.
fn all(storage: &dyn Storage) -> Vec<(String, Option<ImageRef>)> {
    let Ok(data) = quireink_sdk::read_file(storage, &file(), 16 * 1024) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|line| {
            let mut f = line.splitn(4, '\t');
            let path = f.next()?.to_string();
            let width = f.next()?.parse().ok()?;
            let height = f.next()?.parse().ok()?;
            let picture = f.next().unwrap_or_default();
            let image = (!picture.is_empty()).then(|| ImageRef {
                path: picture.to_string(),
                width,
                height,
            });
            Some((path, image))
        })
        .collect()
}

/// The cover of `path`: `None` if not looked up yet, `Some(None)` if it
/// has none.
pub(crate) fn get(storage: &dyn Storage, path: &str) -> Option<Option<ImageRef>> {
    all(storage)
        .into_iter()
        .find(|(p, _)| p == path)
        .map(|(_, image)| image.filter(|i| storage.exists(&i.path)))
}

/// Looks up the cover of `path` (opening the book) and keeps where it is.
pub(crate) fn look_up(env: &Env<'_>, path: &str) -> Option<ImageRef> {
    let cover = env
        .formats
        .open(env.storage, path)
        .ok()
        .and_then(|document| document.cover(env.storage).ok().flatten());
    let mut all = all(env.storage);
    all.retain(|(p, _)| p != path);
    all.insert(0, (path.to_string(), cover.clone()));
    all.truncate(LIMIT);
    let text: String = all
        .iter()
        .map(|(p, image)| match image {
            Some(i) => format!("{p}\t{}\t{}\t{}\n", i.width, i.height, i.path),
            None => format!("{p}\t0\t0\t\n"),
        })
        .collect();
    let _ = env.storage.write(&file(), text.as_bytes());
    cover
}
