//! Syncing catalogs: their books that are not on the device yet are
//! downloaded into each catalog's sync folder, page after page of the
//! catalog's list, at most [`LIMIT`] books a catalog each time.
//!
//! Every book synced is remembered (by its id in the catalog), so a book
//! the user deleted is not fetched again. Books already in the folder
//! under the same name are skipped too.
//!
//! It runs in the background: the answers come as events to the plugin's
//! subscriptions, whatever screen is open. One book is downloaded at a
//! time, straight to storage.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::web::{clean_name, file_name};
use quireink_sdk::{Command, Env, HttpRequest, Id, Params, files, http_params, md5};

use crate::catalogs::Catalog;
use crate::{PLUGIN, feed};

/// Answers to the sync's requests.
pub(crate) const FEED_LOADED: Id = Id::new_static("opds.sync-feed");
pub(crate) const BOOK_LOADED: Id = Id::new_static("opds.sync-book");
/// Emitted whenever a sync gets on or ends, for screens to show.
pub(crate) const CHANGED: Id = Id::new_static("opds.sync-changed");
/// Setting: the most books a catalog gets in one sync.
pub(crate) const LIMIT: Id = Id::new_static("opds.sync-limit");

/// Pages of a catalog's list read in one sync, at most.
const MAX_PAGES: usize = 50;
/// Parameter of the requests: which run they belong to.
const RUN: &str = "run";

/// Where the sync keeps what it is loading.
fn feed_path() -> String {
    format!("{}/sync.xml", Env::plugin_dir(PLUGIN.as_str()))
}

fn book_path() -> String {
    format!("{}/sync.tmp", Env::plugin_dir(PLUGIN.as_str()))
}

/// The books synced from the catalog at `url`, one id a line.
fn record_path(url: &str) -> String {
    format!(
        "{}/synced/{}.txt",
        Env::plugin_dir(PLUGIN.as_str()),
        md5::of(url)
    )
}

/// What a sync did, for the screens.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    /// The catalog being synced, or the last one.
    pub catalog: String,
    pub loaded: usize,
    pub failed: usize,
    /// Catalogs that could not be read.
    pub unreachable: usize,
    /// Catalogs whose address lists no books, only further lists (such
    /// as Project Gutenberg's start page): nothing to sync there.
    pub only_lists: usize,
    pub running: bool,
}

/// A book to download.
#[derive(Debug, Clone)]
struct Item {
    id: String,
    name: String,
    url: String,
}

#[derive(Debug)]
struct Run {
    /// Its number: answers to an earlier run are ignored.
    number: u32,
    catalog: Catalog,
    /// The catalogs after this one.
    waiting: VecDeque<Catalog>,
    folder: String,
    queue: VecDeque<Item>,
    /// The page of the list after the one read.
    next: Option<String>,
    /// The pages read.
    pages: Vec<String>,
    /// Ids of the books synced before.
    synced: Vec<String>,
    /// Books of this catalog loaded in this run.
    count: usize,
    limit: usize,
}

/// The sync of the plugin, shared by its screens and subscriptions.
#[derive(Debug, Default)]
pub(crate) struct Sync {
    run: Option<Run>,
    runs: u32,
    pub report: Option<Report>,
}

impl Sync {
    pub(crate) fn running(&self) -> bool {
        self.run.is_some()
    }

    /// Syncs `catalogs`, one after the other, unless a sync is running.
    pub(crate) fn start(&mut self, env: &Env<'_>, catalogs: Vec<Catalog>, out: &mut Vec<Command>) {
        if self.run.is_some() {
            return;
        }
        let mut waiting: VecDeque<Catalog> = catalogs.into();
        let Some(catalog) = waiting.pop_front() else {
            return;
        };
        self.runs += 1;
        self.report = Some(Report {
            running: true,
            ..Report::default()
        });
        self.begin(env, catalog, waiting, out);
    }

    /// Stops the sync; the books loaded stay.
    pub(crate) fn stop(&mut self, out: &mut Vec<Command>) {
        if self.run.take().is_some() {
            if let Some(report) = &mut self.report {
                report.running = false;
            }
            out.push(Command::Emit(CHANGED, Params::new()));
        }
    }

    fn begin(
        &mut self,
        env: &Env<'_>,
        catalog: Catalog,
        waiting: VecDeque<Catalog>,
        out: &mut Vec<Command>,
    ) {
        let synced = quireink_sdk::read_file(env.storage, &record_path(&catalog.url), 1024 * 1024)
            .map(|data| {
                String::from_utf8_lossy(&data)
                    .lines()
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(report) = &mut self.report {
            report.catalog = catalog.name.clone();
        }
        let limit = env.settings.int(LIMIT.as_str()).max(1) as usize;
        let run = Run {
            number: self.runs,
            folder: catalog.sync_folder(env),
            waiting,
            queue: VecDeque::new(),
            next: None,
            pages: Vec::new(),
            synced,
            count: 0,
            limit,
            catalog,
        };
        let url = run.catalog.url.clone();
        self.run = Some(run);
        self.request_feed(&url, out);
        out.push(Command::Emit(CHANGED, Params::new()));
    }

    fn request(&self, url: &str, save_to: String, reply: Id) -> Option<HttpRequest> {
        let run = self.run.as_ref()?;
        let params = Params::new().with(RUN, run.number.to_string());
        Some(crate::request(&run.catalog, url, save_to, reply, params))
    }

    fn request_feed(&mut self, url: &str, out: &mut Vec<Command>) {
        if let Some(run) = &mut self.run {
            run.pages.push(url.to_string());
        }
        if let Some(mut request) = self.request(url, feed_path(), FEED_LOADED) {
            request.params = request.params.with(crate::URL, url.to_string());
            out.push(Command::Http(request));
        }
    }

    /// Whether `params` answer a request of the run going on.
    fn current(&self, params: &Params) -> bool {
        let number = params.get(RUN).and_then(|n| n.parse::<u32>().ok());
        self.run
            .as_ref()
            .is_some_and(|run| Some(run.number) == number)
    }

    /// A page of the catalog's list arrived.
    pub(crate) fn feed_loaded(&mut self, env: &Env<'_>, params: &Params, out: &mut Vec<Command>) {
        if !self.current(params) {
            return;
        }
        let status = status(params);
        if !(200..300).contains(&status) {
            let _ = env.storage.remove(&feed_path());
            let first = self.run.as_ref().is_some_and(|run| run.pages.len() <= 1);
            if first && let Some(report) = &mut self.report {
                report.unreachable += 1;
            }
            return self.next_catalog(env, out);
        }
        let base = params
            .get(http_params::FINAL_URL)
            .or(params.get(crate::URL))
            .unwrap_or_default()
            .to_string();
        let page = feed::read(env.storage, &feed_path(), &base);
        let _ = env.storage.remove(&feed_path());
        let Some(run) = &mut self.run else {
            return;
        };
        // Sync follows no further lists: a catalog of lists has no books.
        let first = run.pages.len() == 1;
        if first
            && !page.entries.is_empty()
            && page.entries.iter().all(|e| e.books.is_empty())
            && let Some(report) = &mut self.report
        {
            report.only_lists += 1;
        }
        for entry in &page.entries {
            let Some((book, ext)) =
                entry.readable_book(|ext| env.formats.supports(&format!("book.{ext}")))
            else {
                continue;
            };
            let id = if entry.id.is_empty() {
                book.url.clone()
            } else {
                entry.id.clone()
            };
            let name = book_name(entry, &book.url, ext);
            let known = run.synced.contains(&id)
                || run
                    .queue
                    .iter()
                    .any(|item| item.id == id || item.name == name)
                || env.storage.exists(&files::join(&run.folder, &name));
            if !known {
                run.queue.push_back(Item {
                    id,
                    name,
                    url: book.url.clone(),
                });
            }
        }
        run.next = page
            .next
            .filter(|next| run.pages.len() < MAX_PAGES && !run.pages.contains(next));
        self.go_on(env, out);
    }

    /// The next book, else the next page, else the next catalog.
    fn go_on(&mut self, env: &Env<'_>, out: &mut Vec<Command>) {
        let Some(run) = &mut self.run else {
            return;
        };
        if run.count >= run.limit {
            return self.next_catalog(env, out);
        }
        if let Some(item) = run.queue.front() {
            let (url, id) = (item.url.clone(), item.id.clone());
            if let Some(mut request) = self.request(&url, book_path(), BOOK_LOADED) {
                request.params = request.params.with(crate::ID, id);
                out.push(Command::Http(request));
            }
            return;
        }
        match run.next.take() {
            Some(next) => self.request_feed(&next, out),
            None => self.next_catalog(env, out),
        }
    }

    /// A book arrived.
    pub(crate) fn book_loaded(&mut self, env: &Env<'_>, params: &Params, out: &mut Vec<Command>) {
        if !self.current(params) {
            return;
        }
        let Some(run) = &mut self.run else {
            return;
        };
        let Some(item) = run.queue.pop_front() else {
            return;
        };
        let tmp = book_path();
        let stored = (200..300).contains(&status(params)) && {
            let _ = env.storage.create_dir(&run.folder);
            let target = files::free_path(env.storage, &run.folder, &item.name);
            files::move_to(env.storage, &tmp, &target).is_ok()
        };
        let report = self.report.get_or_insert_with(Report::default);
        if stored {
            run.count += 1;
            run.synced.push(item.id.clone());
            let _ = env.storage.append(
                &record_path(&run.catalog.url),
                format!("{}\n", item.id.replace(['\n', '\r'], " ")).as_bytes(),
            );
            report.loaded += 1;
        } else {
            let _ = env.storage.remove(&tmp);
            report.failed += 1;
        }
        out.push(Command::Emit(CHANGED, Params::new()));
        self.go_on(env, out);
    }

    fn next_catalog(&mut self, env: &Env<'_>, out: &mut Vec<Command>) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        match run.waiting.pop_front() {
            Some(catalog) => self.begin(env, catalog, core::mem::take(&mut run.waiting), out),
            None => {
                if let Some(report) = &mut self.report {
                    report.running = false;
                    out.push(Command::Notify(crate::list::report_text(env, report)));
                }
                out.push(Command::Emit(CHANGED, Params::new()));
            }
        }
    }
}

/// The status of an answer; 0 if there was none.
pub(crate) fn status(params: &Params) -> u16 {
    params
        .get(http_params::STATUS)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// The file name for a book: its authors and title, else the name in its
/// address.
pub(crate) fn book_name(entry: &feed::Entry, url: &str, ext: &str) -> String {
    let title = entry.title.trim();
    let name = match entry.authors.split(" & ").next().map(str::trim) {
        Some(author) if !author.is_empty() && !title.is_empty() => format!("{author} - {title}"),
        _ => title.to_string(),
    };
    match clean_name(&name, 120) {
        Some(name) => format!("{name}.{ext}"),
        None => {
            let name = file_name(url);
            if name.ends_with(&format!(".{ext}")) {
                name
            } else {
                format!("{name}.{ext}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Entry;

    #[test]
    fn books_are_named_after_author_and_title() {
        let entry = Entry {
            title: String::from("Pride and Prejudice"),
            authors: String::from("Austen, Jane & Someone Else"),
            ..Entry::default()
        };
        assert_eq!(
            book_name(&entry, "https://a.org/1342.epub.noimages", "epub"),
            "Austen, Jane - Pride and Prejudice.epub"
        );
        let untitled = Entry::default();
        assert_eq!(
            book_name(&untitled, "https://a.org/get/1342", "epub"),
            "1342.epub"
        );
        assert_eq!(
            book_name(&untitled, "https://a.org/b.epub", "epub"),
            "b.epub"
        );
    }
}
