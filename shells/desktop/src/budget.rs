//! A global allocator that enforces a device's heap budget on the kernel.
//!
//! Only allocations made while [`track`] is active — i.e. inside kernel and
//! plugin calls — are counted, so the emulator's own window and graphics
//! code does not distort the numbers. When a tracked allocation would exceed
//! the budget it fails, exactly as it would on the device.
//!
//! The numbers approximate a real embedded allocator: each allocation is
//! charged a small fixed header. Fragmentation and stack usage are not
//! modelled, so keep a safety margin.

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Per-allocation bookkeeping overhead of typical embedded allocators.
const HEADER: usize = 8;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static FAILED: AtomicUsize = AtomicUsize::new(0);
/// Allocations of at least this many bytes are printed.
static TRACE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Whether live allocations are recorded with their call site.
static PROFILE: AtomicBool = AtomicBool::new(false);
/// Live tracked allocations: address → (size, where it was made).
static SITES: Mutex<Option<HashMap<usize, (usize, Backtrace)>>> = Mutex::new(None);

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

/// The emulator's global allocator.
#[derive(Debug)]
pub struct BudgetAllocator;

fn tracking() -> bool {
    TRACKING.try_with(Cell::get).unwrap_or(false)
}

// SAFETY: forwards to the system allocator; the bookkeeping does not touch
// the allocated memory.
unsafe impl GlobalAlloc for BudgetAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !tracking() {
            // SAFETY: same contract as the caller's.
            return unsafe { System.alloc(layout) };
        }
        let charge = layout.size() + HEADER;
        if LIVE.load(Ordering::Relaxed) + charge > LIMIT.load(Ordering::Relaxed) {
            FAILED.fetch_add(1, Ordering::Relaxed);
            return std::ptr::null_mut();
        }
        // SAFETY: same contract as the caller's.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(charge, Ordering::Relaxed) + charge;
            PEAK.fetch_max(live, Ordering::Relaxed);
            if layout.size() >= TRACE.load(Ordering::Relaxed) {
                // Untracked, so printing does not count against the budget.
                let tracking = TRACKING.with(|t| t.replace(false));
                eprintln!(
                    "alloc {:>7} B, heap {:>6.1} KiB",
                    layout.size(),
                    live as f64 / 1024.0
                );
                TRACKING.with(|t| t.set(tracking));
            }
            if PROFILE.load(Ordering::Relaxed) {
                // Untracked: the bookkeeping is not part of the budget.
                let tracking = TRACKING.with(|t| t.replace(false));
                let site = Backtrace::force_capture();
                if let Ok(mut sites) = SITES.lock() {
                    sites
                        .get_or_insert_with(HashMap::new)
                        .insert(ptr as usize, (layout.size(), site));
                }
                TRACKING.with(|t| t.set(tracking));
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) };
        if tracking() {
            if PROFILE.load(Ordering::Relaxed) {
                let tracking = TRACKING.with(|t| t.replace(false));
                if let Ok(mut sites) = SITES.lock()
                    && let Some(sites) = sites.as_mut()
                {
                    sites.remove(&(ptr as usize));
                }
                TRACKING.with(|t| t.set(tracking));
            }
            let charge = layout.size() + HEADER;
            let mut live = LIVE.load(Ordering::Relaxed);
            while let Err(actual) = LIVE.compare_exchange_weak(
                live,
                live.saturating_sub(charge),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                live = actual;
            }
        }
    }
}

/// Runs `f` with allocation tracking enabled.
pub fn track<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            TRACKING.with(|t| t.set(self.0));
        }
    }
    let _restore = Restore(TRACKING.with(|t| t.replace(true)));
    f()
}

/// Sets the heap budget; `None` removes it.
pub fn set_limit(limit: Option<usize>) {
    LIMIT.store(limit.unwrap_or(usize::MAX), Ordering::Relaxed);
}

/// Prints every tracked allocation of at least `bytes` to stderr, to find
/// what makes up a peak.
pub fn trace(bytes: usize) {
    TRACE.store(bytes, Ordering::Relaxed);
}

/// Records the call site of every tracked allocation for [`report`].
/// Slow; for analysing memory use only.
pub fn profile() {
    PROFILE.store(true, Ordering::Relaxed);
}

/// The first frames in QuireInk code below the allocator, e.g.
/// `quireink_text::cache::GlyphCache::insert (cache.rs:120)`.
fn site_of(backtrace: &Backtrace, depth: usize) -> String {
    let text = backtrace.to_string();
    let mut frames = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((_, name)) = line.trim().split_once(": ") else {
            continue;
        };
        let location = lines
            .peek()
            .and_then(|l| l.trim().strip_prefix("at "))
            .map(|l| {
                let file = l.rsplit(['\\', '/']).next().unwrap_or(l);
                file.rsplitn(2, ':').nth(1).unwrap_or(file).to_string()
            })
            .unwrap_or_default();
        let name = name.rsplit_once("::h").map_or(name, |(n, _)| n);
        if name.starts_with("quireink") && !name.contains("budget") && !name.contains("__rust_") {
            frames.push(format!("{name} ({location})"));
            if frames.len() == depth {
                break;
            }
        } else if !frames.is_empty() && name.contains("quireink_emu") {
            break;
        }
    }
    if frames.is_empty() {
        String::from("(outside QuireInk code)")
    } else {
        frames.join(
            "
      <- ",
        )
    }
}

/// Prints the live allocations grouped by call site, largest first.
pub fn report(depth: usize) {
    let tracking = TRACKING.with(|t| t.replace(false));
    let sites = SITES
        .lock()
        .ok()
        .and_then(|mut s| s.take())
        .unwrap_or_default();
    let mut groups: HashMap<String, (usize, usize)> = HashMap::new();
    for (size, backtrace) in sites.values() {
        let group = groups.entry(site_of(backtrace, depth)).or_default();
        group.0 += size + HEADER;
        group.1 += 1;
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    let total: usize = groups.iter().map(|(_, (bytes, _))| bytes).sum();
    println!("live heap by call site ({:.1} KiB):", total as f64 / 1024.0);
    for (site, (bytes, count)) in groups {
        println!("{:>8.1} KiB {count:>6}x  {site}", bytes as f64 / 1024.0);
    }
    TRACKING.with(|t| t.set(tracking));
}

/// Current heap usage of the kernel and its plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub live: usize,
    pub peak: usize,
    pub limit: Option<usize>,
    /// Allocations refused because of the budget.
    pub failed: usize,
}

pub fn stats() -> Stats {
    let limit = LIMIT.load(Ordering::Relaxed);
    Stats {
        live: LIVE.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
        limit: (limit != usize::MAX).then_some(limit),
        failed: FAILED.load(Ordering::Relaxed),
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kib = |bytes: usize| bytes as f64 / 1024.0;
        write!(f, "heap {:.1} KiB", kib(self.live))?;
        if let Some(limit) = self.limit {
            write!(f, " / {:.0} KiB", kib(limit))?;
        }
        write!(f, " (peak {:.1} KiB)", kib(self.peak))
    }
}
