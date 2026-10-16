use quireink_platform::{Point, PointerTool};

/// Direction of a swipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const fn id(self) -> &'static str {
        match self {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GestureKind {
    Tap,
    LongPress,
    Swipe(Direction),
    /// Two fingers moved apart (`out`) or together: in readers the text
    /// larger or smaller.
    Pinch {
        out: bool,
    },
}

/// A gesture recognized by the kernel from raw pointer samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Gesture {
    pub kind: GestureKind,
    /// Where the gesture started.
    pub start: Point,
    /// Where it ended (equal to `start` for taps).
    pub end: Point,
    pub tool: PointerTool,
}
