//! What each plugin takes (see [`quireink_sdk::usage::PluginUsage`]): the
//! time its code runs (its screens drawing and handling input, its event
//! and action handlers, opening its screens), the storage read and written
//! meanwhile, and the time its screen is on top with the refreshes then.
//! The kernel itself counts as `core`.
//!
//! Time is measured with the shell's clock
//! ([`KernelBuilder::clock`](crate::KernelBuilder::clock)); without one,
//! only what needs none is counted.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_platform::{DirEntry, Rect, RefreshMode, Storage, StorageError};
use quireink_sdk::Id;
use quireink_sdk::usage::PluginUsage;

/// What the kernel's own work counts as.
pub(crate) const CORE: &str = "core";

pub(crate) struct Meter {
    clock: Option<fn() -> u64>,
    /// By plugin, `core` first.
    table: RefCell<Vec<PluginUsage>>,
    /// The plugins whose code runs, the innermost last, with when it began
    /// (or went on after an inner one), in microseconds.
    running: RefCell<Vec<(usize, u64)>>,
}

impl core::fmt::Debug for Meter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Meter").finish_non_exhaustive()
    }
}

/// A plugin's code running, until dropped.
pub(crate) struct Running(Rc<Meter>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.leave();
    }
}

impl Meter {
    pub(crate) fn new(clock: Option<fn() -> u64>) -> Rc<Self> {
        let meter = Rc::new(Self {
            clock,
            table: RefCell::new(Vec::new()),
            running: RefCell::new(Vec::new()),
        });
        meter.index(CORE);
        meter
    }

    fn now(&self) -> u64 {
        self.clock.map_or(0, |clock| clock())
    }

    /// The row of `plugin`, added the first time.
    fn index(&self, plugin: &str) -> usize {
        let mut table = self.table.borrow_mut();
        if let Some(i) = table.iter().position(|p| p.plugin.as_str() == plugin) {
            return i;
        }
        let id = Id::parse(plugin).unwrap_or(Id::new_static(CORE));
        table.push(PluginUsage {
            plugin: id,
            ..PluginUsage::default()
        });
        table.len() - 1
    }

    /// `plugin`'s code runs until the result is dropped; the code it was
    /// called from pauses meanwhile.
    pub(crate) fn enter(self: &Rc<Self>, plugin: &str) -> Running {
        let index = self.index(plugin);
        let now = self.now();
        let mut running = self.running.borrow_mut();
        if let Some(&(outer, since)) = running.last() {
            self.add_busy(outer, now.saturating_sub(since));
        }
        running.push((index, now));
        self.table.borrow_mut()[index].calls += 1;
        Running(self.clone())
    }

    fn leave(&self) {
        let now = self.now();
        let mut running = self.running.borrow_mut();
        if let Some((index, since)) = running.pop() {
            self.add_busy(index, now.saturating_sub(since));
        }
        if let Some(last) = running.last_mut() {
            last.1 = now;
        }
    }

    fn add_busy(&self, index: usize, micros: u64) {
        if let Some(p) = self.table.borrow_mut().get_mut(index) {
            p.busy_us = p.busy_us.saturating_add(micros);
        }
    }

    /// Storage used by the code running now.
    fn io(&self, read: usize, written: usize) {
        let index = self.running.borrow().last().map_or(0, |&(i, _)| i);
        if let Some(p) = self.table.borrow_mut().get_mut(index) {
            p.read = p.read.saturating_add(read as u64);
            p.written = p.written.saturating_add(written as u64);
        }
    }

    /// `plugin`'s screen was on top for `millis`.
    pub(crate) fn shown(&self, plugin: &str, millis: u64) {
        let index = self.index(plugin);
        let mut table = self.table.borrow_mut();
        table[index].shown_ms = table[index].shown_ms.saturating_add(millis);
    }

    /// A refresh while `plugin`'s screen was on top.
    pub(crate) fn refreshed(&self, plugin: &str, mode: RefreshMode, area: Rect) {
        let index = self.index(plugin);
        let mut table = self.table.borrow_mut();
        let p = &mut table[index];
        let at = match mode {
            RefreshMode::Fast => 0,
            RefreshMode::Partial => 1,
            RefreshMode::Full => 2,
        };
        p.refreshes[at] = p.refreshes[at].saturating_add(1);
        p.pixels = p
            .pixels
            .saturating_add(u64::from(area.width) * u64::from(area.height));
    }

    /// Every plugin's numbers so far.
    pub(crate) fn snapshot(&self, into: &mut Vec<PluginUsage>) {
        into.clone_from(&self.table.borrow());
    }
}

/// The storage, with what is read and written counted for the code
/// running.
pub(crate) struct Metered {
    pub inner: Box<dyn Storage>,
    pub meter: Rc<Meter>,
}

impl Storage for Metered {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        let n = self.inner.read_at(path, offset, buf)?;
        self.meter.io(n, 0);
        Ok(n)
    }

    fn size(&self, path: &str) -> Result<u64, StorageError> {
        self.inner.size(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.meter.io(0, data.len());
        self.inner.write(path, data)
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.meter.io(0, data.len());
        self.inner.append(path, data)
    }

    fn sync(&self) -> Result<(), StorageError> {
        self.inner.sync()
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        self.inner.list(dir, visit)
    }

    fn remove(&self, path: &str) -> Result<(), StorageError> {
        self.inner.remove(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.inner.rename(from, to)
    }

    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        self.inner.create_dir(path)
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), StorageError> {
        self.inner.remove_dir_all(path)
    }

    fn local_path(&self, path: &str, with: &mut dyn FnMut(&str)) {
        self.inner.local_path(path, with);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};

    static NOW: AtomicU64 = AtomicU64::new(0);

    fn clock() -> u64 {
        NOW.load(Ordering::Relaxed)
    }

    #[test]
    fn inner_code_is_its_own_not_the_outer_ones() {
        let meter = Meter::new(Some(clock));
        NOW.store(100, Ordering::Relaxed);
        let outer = meter.enter("reader");
        NOW.store(130, Ordering::Relaxed);
        {
            let _inner = meter.enter("stats");
            meter.io(10, 0);
            NOW.store(180, Ordering::Relaxed);
        }
        meter.io(5, 7);
        NOW.store(200, Ordering::Relaxed);
        drop(outer);
        // Outside every plugin: the kernel's.
        meter.io(1, 0);
        meter.refreshed("reader", RefreshMode::Partial, Rect::new(0, 0, 10, 20));
        meter.shown("reader", 1500);
        let mut table = Vec::new();
        meter.snapshot(&mut table);
        let row = |id: &str| table.iter().find(|p| p.plugin.as_str() == id).unwrap();
        assert_eq!(row("reader").busy_us, 30 + 20);
        assert_eq!(row("stats").busy_us, 50);
        assert_eq!((row("stats").read, row("reader").read), (10, 5));
        assert_eq!((row("reader").written, row("core").read), (7, 1));
        assert_eq!(row("reader").refreshes, [0, 1, 0]);
        assert_eq!((row("reader").pixels, row("reader").shown_ms), (200, 1500));
        assert_eq!((row("reader").calls, row("stats").calls), (1, 1));
    }
}
