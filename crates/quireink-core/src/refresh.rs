//! Decides how the panel is refreshed.

use quireink_platform::{Rect, RefreshMode};

/// Upgrades every n-th partial refresh to a full refresh of the whole
/// screen to remove ghosting.
#[derive(Debug, Default)]
pub(crate) struct RefreshPolicy {
    partials: u32,
}

impl RefreshPolicy {
    /// Returns the mode and region to refresh. `interval == 0` disables the
    /// periodic full refresh.
    pub(crate) fn decide(
        &mut self,
        requested: RefreshMode,
        dirty: Rect,
        screen: Rect,
        interval: u32,
    ) -> (RefreshMode, Rect) {
        match requested {
            RefreshMode::Full => {
                self.partials = 0;
                (RefreshMode::Full, dirty)
            }
            RefreshMode::Partial => {
                self.partials += 1;
                if interval > 0 && self.partials >= interval {
                    self.partials = 0;
                    (RefreshMode::Full, screen)
                } else {
                    (RefreshMode::Partial, dirty)
                }
            }
            RefreshMode::Fast => (RefreshMode::Fast, dirty),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_nth_partial_becomes_full() {
        let mut policy = RefreshPolicy::default();
        let dirty = Rect::new(1, 1, 2, 2);
        let screen = Rect::new(0, 0, 10, 10);
        let modes: [RefreshMode; 4] =
            core::array::from_fn(|_| policy.decide(RefreshMode::Partial, dirty, screen, 3).0);
        assert_eq!(
            modes,
            [
                RefreshMode::Partial,
                RefreshMode::Partial,
                RefreshMode::Full,
                RefreshMode::Partial
            ]
        );
    }

    #[test]
    fn fast_does_not_count_and_zero_disables() {
        let mut policy = RefreshPolicy::default();
        let r = Rect::new(0, 0, 1, 1);
        for _ in 0..10 {
            assert_eq!(
                policy.decide(RefreshMode::Fast, r, r, 2).0,
                RefreshMode::Fast
            );
            assert_eq!(
                policy.decide(RefreshMode::Partial, r, r, 0).0,
                RefreshMode::Partial
            );
        }
    }
}
