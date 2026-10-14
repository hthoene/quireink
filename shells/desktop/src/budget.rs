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
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Per-allocation bookkeeping overhead of typical embedded allocators.
const HEADER: usize = 8;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static FAILED: AtomicUsize = AtomicUsize::new(0);

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
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) };
        if tracking() {
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
