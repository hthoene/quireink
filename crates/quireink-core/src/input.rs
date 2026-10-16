//! Recognizes long presses and gestures from raw input.

use alloc::vec::Vec;

use quireink_platform::{ButtonState, Instant, Point, PointerEvent, PointerPhase, PointerTool};
use quireink_sdk::{Direction, Gesture, GestureKind};

/// Recognition thresholds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Thresholds {
    pub long_press_ms: u64,
    /// Movement in pixels below which a touch still counts as a tap.
    pub tap_slop: u32,
    /// Movement in pixels from which a touch counts as a swipe.
    pub swipe_distance: u32,
}

/// The result of feeding an event or a tick into the recognizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recognized {
    Button { name: &'static str, long: bool },
    Gesture(Gesture),
}

#[derive(Debug)]
struct HeldButton {
    name: &'static str,
    since: Instant,
    long_fired: bool,
}

#[derive(Debug)]
struct TrackedPointer {
    id: u8,
    start: Point,
    last: Point,
    since: Instant,
    tool: PointerTool,
    moved: bool,
    long_fired: bool,
    /// Lifted as part of two fingers: no tap or swipe of its own.
    paired: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Recognizer {
    buttons: Vec<HeldButton>,
    pointers: Vec<TrackedPointer>,
}

impl Recognizer {
    pub(crate) fn button(
        &mut self,
        name: &'static str,
        state: ButtonState,
        now: Instant,
    ) -> Option<Recognized> {
        let index = self.buttons.iter().position(|b| b.name == name);
        match (state, index) {
            (ButtonState::Pressed, None) => {
                self.buttons.push(HeldButton {
                    name,
                    since: now,
                    long_fired: false,
                });
                None
            }
            // Key repeat or a missed release: keep the original press time.
            (ButtonState::Pressed, Some(_)) => None,
            (ButtonState::Released, Some(i)) => {
                let held = self.buttons.swap_remove(i);
                (!held.long_fired).then_some(Recognized::Button { name, long: false })
            }
            (ButtonState::Released, None) => None,
        }
    }

    /// Tracks a pointer the screen did not capture. Returns a gesture when
    /// the pointer is lifted.
    pub(crate) fn pointer(
        &mut self,
        event: &PointerEvent,
        now: Instant,
        thresholds: &Thresholds,
    ) -> Option<Recognized> {
        let index = self.pointers.iter().position(|p| p.id == event.id);
        match (event.phase, index) {
            (PointerPhase::Down, _) => {
                if let Some(i) = index {
                    self.pointers.swap_remove(i);
                }
                self.pointers.push(TrackedPointer {
                    id: event.id,
                    start: event.point,
                    last: event.point,
                    since: now,
                    tool: event.tool,
                    moved: false,
                    long_fired: false,
                    paired: false,
                });
                None
            }
            (PointerPhase::Move, Some(i)) => {
                let pointer = &mut self.pointers[i];
                pointer.last = event.point;
                if distance(pointer.start, event.point) > thresholds.tap_slop {
                    pointer.moved = true;
                }
                None
            }
            (PointerPhase::Up, Some(i)) => {
                let mut pointer = self.pointers.swap_remove(i);
                pointer.last = event.point;
                if distance(pointer.start, event.point) > thresholds.tap_slop {
                    pointer.moved = true;
                }
                if pointer.long_fired || pointer.paired {
                    return None;
                }
                // Two fingers: a pinch if they moved apart or together;
                // the other one then makes no gesture of its own.
                if let Some(other) = self.pointers.iter_mut().find(|p| !p.paired) {
                    other.paired = true;
                    let before = distance(pointer.start, other.start).max(1);
                    let now = distance(pointer.last, other.last);
                    let kind = if now * 4 >= before * 5 {
                        GestureKind::Pinch { out: true }
                    } else if now * 5 <= before * 4 {
                        GestureKind::Pinch { out: false }
                    } else {
                        return None;
                    };
                    return Some(Recognized::Gesture(Gesture {
                        kind,
                        start: pointer.start,
                        end: pointer.last,
                        tool: pointer.tool,
                    }));
                }
                let kind = if !pointer.moved {
                    GestureKind::Tap
                } else if distance(pointer.start, pointer.last) >= thresholds.swipe_distance {
                    GestureKind::Swipe(direction(pointer.start, pointer.last))
                } else {
                    return None;
                };
                Some(Recognized::Gesture(Gesture {
                    kind,
                    start: pointer.start,
                    end: pointer.last,
                    tool: pointer.tool,
                }))
            }
            (PointerPhase::Cancel, Some(i)) => {
                self.pointers.swap_remove(i);
                None
            }
            (PointerPhase::Move | PointerPhase::Up | PointerPhase::Cancel, None) => None,
        }
    }

    /// Fires long presses that are due at `now`.
    pub(crate) fn tick(&mut self, now: Instant, thresholds: &Thresholds) -> Vec<Recognized> {
        let mut fired = Vec::new();
        // Two fingers down: no long press, maybe a pinch.
        let together = self.pointers.len() > 1;
        for button in &mut self.buttons {
            if !button.long_fired && now.since(button.since) >= thresholds.long_press_ms {
                button.long_fired = true;
                fired.push(Recognized::Button {
                    name: button.name,
                    long: true,
                });
            }
        }
        for pointer in &mut self.pointers {
            if !pointer.long_fired
                && !pointer.moved
                && !together
                && now.since(pointer.since) >= thresholds.long_press_ms
            {
                pointer.long_fired = true;
                fired.push(Recognized::Gesture(Gesture {
                    kind: GestureKind::LongPress,
                    start: pointer.start,
                    end: pointer.last,
                    tool: pointer.tool,
                }));
            }
        }
        fired
    }

    /// When the next long press could fire, if any.
    pub(crate) fn next_deadline(&self, thresholds: &Thresholds) -> Option<Instant> {
        let buttons = self
            .buttons
            .iter()
            .filter(|b| !b.long_fired)
            .map(|b| b.since);
        let pointers = self
            .pointers
            .iter()
            .filter(|p| !p.long_fired && !p.moved)
            .map(|p| p.since);
        buttons
            .chain(pointers)
            .min()
            .map(|since| since.add_millis(thresholds.long_press_ms))
    }
}

pub(crate) fn distance(a: Point, b: Point) -> u32 {
    let dx = i64::from(a.x) - i64::from(b.x);
    let dy = i64::from(a.y) - i64::from(b.y);
    isqrt((dx * dx + dy * dy) as u64) as u32
}

fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

pub(crate) fn direction(from: Point, to: Point) -> Direction {
    let dx = i64::from(to.x) - i64::from(from.x);
    let dy = i64::from(to.y) - i64::from(from.y);
    if dx.abs() >= dy.abs() {
        if dx < 0 {
            Direction::Left
        } else {
            Direction::Right
        }
    } else if dy < 0 {
        Direction::Up
    } else {
        Direction::Down
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds {
        long_press_ms: 500,
        tap_slop: 10,
        swipe_distance: 50,
    };

    fn pointer(phase: PointerPhase, x: i32, y: i32) -> PointerEvent {
        PointerEvent {
            id: 0,
            phase,
            point: Point::new(x, y),
            tool: PointerTool::Finger,
            pressure: 1000,
        }
    }

    #[test]
    fn short_and_long_button_presses() {
        let mut r = Recognizer::default();
        assert_eq!(r.button("confirm", ButtonState::Pressed, Instant(0)), None);
        assert_eq!(
            r.button("confirm", ButtonState::Released, Instant(100)),
            Some(Recognized::Button {
                name: "confirm",
                long: false
            })
        );

        r.button("confirm", ButtonState::Pressed, Instant(1000));
        assert_eq!(r.next_deadline(&T), Some(Instant(1500)));
        assert!(r.tick(Instant(1400), &T).is_empty());
        assert_eq!(
            r.tick(Instant(1500), &T),
            [Recognized::Button {
                name: "confirm",
                long: true
            }]
        );
        assert_eq!(r.next_deadline(&T), None);
        // Releasing after a long press produces nothing more.
        assert_eq!(
            r.button("confirm", ButtonState::Released, Instant(1600)),
            None
        );
    }

    #[test]
    fn tap_swipe_and_long_press() {
        let mut r = Recognizer::default();
        r.pointer(&pointer(PointerPhase::Down, 100, 100), Instant(0), &T);
        let tap = r.pointer(&pointer(PointerPhase::Up, 104, 98), Instant(80), &T);
        assert!(matches!(
            tap,
            Some(Recognized::Gesture(Gesture {
                kind: GestureKind::Tap,
                ..
            }))
        ));

        r.pointer(&pointer(PointerPhase::Down, 300, 100), Instant(0), &T);
        r.pointer(&pointer(PointerPhase::Move, 200, 110), Instant(50), &T);
        let swipe = r.pointer(&pointer(PointerPhase::Up, 150, 112), Instant(90), &T);
        assert!(matches!(
            swipe,
            Some(Recognized::Gesture(Gesture {
                kind: GestureKind::Swipe(Direction::Left),
                ..
            }))
        ));

        r.pointer(&pointer(PointerPhase::Down, 10, 10), Instant(0), &T);
        let fired = r.tick(Instant(600), &T);
        assert!(matches!(
            fired.as_slice(),
            [Recognized::Gesture(Gesture {
                kind: GestureKind::LongPress,
                ..
            })]
        ));
        assert_eq!(
            r.pointer(&pointer(PointerPhase::Up, 10, 10), Instant(700), &T),
            None
        );
    }

    fn finger(id: u8, phase: PointerPhase, x: i32, y: i32) -> PointerEvent {
        PointerEvent {
            id,
            ..pointer(phase, x, y)
        }
    }

    #[test]
    fn two_fingers_pinch() {
        let mut r = Recognizer::default();
        for (out, from, to) in [(false, 40, 10), (true, 10, 40)] {
            r.pointer(
                &finger(1, PointerPhase::Down, 200 - from, 300),
                Instant(0),
                &T,
            );
            r.pointer(
                &finger(2, PointerPhase::Down, 200 + from, 300),
                Instant(10),
                &T,
            );
            // Held long, but two fingers make no long press.
            assert!(r.tick(Instant(600), &T).is_empty());
            r.pointer(
                &finger(1, PointerPhase::Move, 200 - to, 300),
                Instant(700),
                &T,
            );
            r.pointer(
                &finger(2, PointerPhase::Move, 200 + to, 300),
                Instant(700),
                &T,
            );
            let pinch = r.pointer(
                &finger(1, PointerPhase::Up, 200 - to, 300),
                Instant(750),
                &T,
            );
            assert!(matches!(
                pinch,
                Some(Recognized::Gesture(Gesture {
                    kind: GestureKind::Pinch { out: o },
                    ..
                })) if o == out
            ));
            // The second finger makes nothing of its own.
            assert_eq!(
                r.pointer(
                    &finger(2, PointerPhase::Up, 200 + to, 300),
                    Instant(760),
                    &T
                ),
                None
            );
        }
        // Two fingers that barely moved: nothing.
        r.pointer(&finger(1, PointerPhase::Down, 100, 300), Instant(0), &T);
        r.pointer(&finger(2, PointerPhase::Down, 300, 300), Instant(0), &T);
        assert_eq!(
            r.pointer(&finger(1, PointerPhase::Up, 102, 300), Instant(80), &T),
            None
        );
        assert_eq!(
            r.pointer(&finger(2, PointerPhase::Up, 300, 300), Instant(90), &T),
            None
        );
    }

    #[test]
    fn small_drags_are_ignored() {
        let mut r = Recognizer::default();
        r.pointer(&pointer(PointerPhase::Down, 0, 0), Instant(0), &T);
        assert_eq!(
            r.pointer(&pointer(PointerPhase::Up, 30, 0), Instant(50), &T),
            None
        );
    }

    #[test]
    fn integer_square_root() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(15), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(u64::from(u32::MAX)), 65535);
    }
}
