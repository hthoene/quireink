//! The home screen's pages as grids, like a phone's home screen: a page is
//! cut into cells (4 columns by default, rows to fit the screen), and each
//! item takes a rectangle of them: a widget (any plugin's) or a tool (an
//! action, shown as its icon and name). Cells left empty stay empty, so
//! everything sits where it was put.
//!
//! A page is a setting of its own (`home.page-1` …): its items separated
//! by commas, each its id, its column and row, and its size in cells,
//! e.g. `home.clock 0/0 4x1, tool:files.library 0/5`. An item without a
//! place (as plugins add them, or pages written before the grid) goes into
//! the first free cells, as wide as the page and as high as it needs.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus};
use quireink_sdk::ui::UiStyle;
use quireink_sdk::{Env, Id, Rect};

use crate::ACTIONS;

/// Columns at most, and rows at most.
pub(crate) const MAX_COLUMNS: u8 = 6;
pub(crate) const MAX_ROWS: u8 = 12;
/// Tools are written with this before their action's id.
const TOOL: &str = "tool:";

/// What an item shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum What {
    /// A widget, by its id.
    Widget(String),
    /// A tool: an action, shown as its icon and name.
    Tool(Id),
}

impl What {
    fn text(&self) -> String {
        match self {
            What::Widget(id) => id.clone(),
            What::Tool(action) => format!("{TOOL}{action}"),
        }
    }
}

/// An item of a page: what it shows and the cells it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub what: What,
    /// Its first column and row, and its width and height in cells;
    /// `None` until it has a place (see [`place`]).
    pub at: Option<Cells>,
}

/// A rectangle of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cells {
    pub col: u8,
    pub row: u8,
    pub w: u8,
    pub h: u8,
}

impl Cells {
    pub fn overlaps(&self, other: &Cells) -> bool {
        self.col < other.col + other.w
            && other.col < self.col + self.w
            && self.row < other.row + other.h
            && other.row < self.row + self.h
    }

    /// Whether it lies within a grid of `columns` by `rows`.
    pub fn fits(&self, columns: u8, rows: u8) -> bool {
        self.w > 0 && self.h > 0 && self.col + self.w <= columns && self.row + self.h <= rows
    }
}

/// The items of a page as its setting has them.
pub(crate) fn parse(text: &str) -> Vec<Item> {
    text.split(',')
        .filter_map(|item| {
            let mut parts = item.split_whitespace();
            let id = parts.next()?;
            let what = match id.strip_prefix(TOOL) {
                Some(action) => What::Tool(Id::parse(action).ok()?),
                None => What::Widget(id.to_string()),
            };
            let at = (|| {
                let (col, row) = parts.next()?.split_once('/')?;
                let (w, h) = parts.next()?.split_once('x')?;
                Some(Cells {
                    col: col.parse().ok()?,
                    row: row.parse().ok()?,
                    w: w.parse().ok()?,
                    h: h.parse().ok()?,
                })
            })();
            Some(Item { what, at })
        })
        .collect()
}

/// The setting of a page with `items`.
pub(crate) fn write(items: &[Item]) -> String {
    let mut out = String::new();
    for item in items {
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&item.what.text());
        if let Some(c) = item.at {
            out.push_str(&format!(" {}/{} {}x{}", c.col, c.row, c.w, c.h));
        }
    }
    out
}

/// The cells of a page on this screen, and where they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub columns: u8,
    pub rows: u8,
    /// The area the cells fill.
    pub area: Rect,
    pub cell_w: u16,
    pub cell_h: u16,
    /// Between items.
    pub gap: u16,
}

impl Geometry {
    /// The grid in `area`, with the columns and rows set (rows 0: as many
    /// as make cells about as high as wide).
    pub fn new(env: &Env<'_>, area: Rect) -> Self {
        let style = UiStyle::new(env);
        let columns = (env
            .settings
            .int(crate::pages::COLUMNS.as_str())
            .clamp(2, i64::from(MAX_COLUMNS))) as u8;
        let cell_w = area.width / u16::from(columns);
        let rows = match env.settings.int(crate::pages::ROWS.as_str()) {
            0 => {
                let target = (u32::from(cell_w) * 85 / 100).max(1);
                ((u32::from(area.height) + target / 2) / target).clamp(3, u32::from(MAX_ROWS)) as u8
            }
            n => n.clamp(2, i64::from(MAX_ROWS)) as u8,
        };
        let cell_h = area.height / u16::from(rows);
        Self {
            columns,
            rows,
            area,
            cell_w,
            cell_h,
            gap: style.gap as u16,
        }
    }

    /// Where the cells `c` are on the screen, the gap left around.
    pub fn rect(&self, c: Cells) -> Rect {
        let half = self.gap / 2;
        Rect::new(
            self.area.x + u16::from(c.col) * self.cell_w + half,
            self.area.y + u16::from(c.row) * self.cell_h + half,
            (u16::from(c.w) * self.cell_w).saturating_sub(self.gap),
            (u16::from(c.h) * self.cell_h).saturating_sub(self.gap),
        )
    }

    /// The cell at a point, if it is on the grid.
    pub fn cell_at(&self, x: i32, y: i32) -> Option<(u8, u8)> {
        let (dx, dy) = (x - i32::from(self.area.x), y - i32::from(self.area.y));
        if dx < 0 || dy < 0 {
            return None;
        }
        let col = dx / i32::from(self.cell_w.max(1));
        let row = dy / i32::from(self.cell_h.max(1));
        (col < i32::from(self.columns) && row < i32::from(self.rows))
            .then_some((col as u8, row as u8))
    }

    /// The size an item gets when it has no place yet: a tool one cell, a
    /// widget the page's width and the rows it needs.
    pub fn size_of(&self, env: &Env<'_>, what: &What) -> (u8, u8) {
        match what {
            What::Tool(_) => (1, 1),
            What::Widget(id) => {
                let width = (u16::from(self.columns) * self.cell_w).saturating_sub(self.gap);
                let height = env.widgets.height(id, env, width).unwrap_or(0);
                let rows = (u32::from(height) + u32::from(self.gap))
                    .div_ceil(u32::from(self.cell_h.max(1)));
                (self.columns, rows.clamp(1, u32::from(self.rows)) as u8)
            }
        }
    }
}

/// The first free place of `w` by `h` cells among `taken`, row by row.
pub(crate) fn free_place(geometry: &Geometry, taken: &[Cells], w: u8, h: u8) -> Option<Cells> {
    for row in 0..geometry.rows {
        for col in 0..geometry.columns {
            let c = Cells { col, row, w, h };
            if c.fits(geometry.columns, geometry.rows) && !taken.iter().any(|t| t.overlaps(&c)) {
                return Some(c);
            }
        }
    }
    None
}

/// Every item with its place: those placed where they are (if they fit
/// the grid and nothing before took the cells), the others in the first
/// free cells. Items without room are left out.
pub(crate) fn place(env: &Env<'_>, geometry: &Geometry, items: &[Item]) -> Vec<(Item, Cells)> {
    let mut placed: Vec<(Item, Cells)> = Vec::new();
    let fits = |c: &Cells, placed: &[(Item, Cells)]| {
        c.fits(geometry.columns, geometry.rows) && !placed.iter().any(|(_, p)| p.overlaps(c))
    };
    // Those with a place first, so the others do not take it.
    for item in items {
        if let Some(c) = item.at
            && fits(&c, &placed)
        {
            placed.push((item.clone(), c));
        }
    }
    for item in items.iter().filter(|i| i.at.is_none()) {
        let (w, h) = geometry.size_of(env, &item.what);
        let taken: Vec<Cells> = placed.iter().map(|(_, c)| *c).collect();
        // Where the full size has no room, a smaller one may.
        let found = (1..=w)
            .rev()
            .find_map(|w| free_place(geometry, &taken, w, h));
        if let Some(c) = found {
            placed.push((item.clone(), c));
        }
    }
    placed
}

/// The tools a page may show: the shortcuts, the tools, then the
/// settings; their action and the translation key of their name.
pub(crate) fn tools(env: &Env<'_>) -> Vec<(Id, String)> {
    let mut items: Vec<(Id, String)> = Vec::new();
    for menu in [ACTIONS, menus::TOOLS] {
        for item in env.menus.items(menu.as_str()) {
            // A screenshot from here shows only the home screen: it is
            // for the tools opened over a book (unless listed above).
            let here_useless = menu == menus::TOOLS && *item.id == actions::SCREENSHOT;
            if !item.section && !here_useless && !items.iter().any(|(id, _)| id == item.id) {
                items.push((item.id.clone(), item.title.to_string()));
            }
        }
    }
    if !items.iter().any(|(id, _)| *id == actions::SETTINGS) {
        items.push((actions::SETTINGS, String::from("core.action.settings")));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_read_and_written() {
        let text = "home.clock 0/0 4x1, tool:core.library 1/5 1x1, stats.today";
        let items = parse(text);
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].what, What::Tool(Id::new_static("core.library")));
        assert_eq!(
            items[0].at,
            Some(Cells {
                col: 0,
                row: 0,
                w: 4,
                h: 1
            })
        );
        assert_eq!(items[2].at, None);
        assert_eq!(write(&items), text);
        // Pages written before the grid: places to come.
        assert!(
            parse("home.status, home.date")
                .iter()
                .all(|i| i.at.is_none())
        );
    }

    #[test]
    fn cells_overlap_and_fit() {
        let a = Cells {
            col: 0,
            row: 0,
            w: 2,
            h: 2,
        };
        assert!(a.overlaps(&Cells {
            col: 1,
            row: 1,
            w: 1,
            h: 1
        }));
        assert!(!a.overlaps(&Cells {
            col: 2,
            row: 0,
            w: 2,
            h: 1
        }));
        assert!(a.fits(4, 6));
        assert!(
            !Cells {
                col: 3,
                row: 0,
                w: 2,
                h: 1
            }
            .fits(4, 6)
        );
    }

    #[test]
    fn free_places_are_found_row_by_row() {
        let g = Geometry {
            columns: 4,
            rows: 3,
            area: Rect::new(0, 0, 400, 300),
            cell_w: 100,
            cell_h: 100,
            gap: 10,
        };
        let taken = [
            Cells {
                col: 0,
                row: 0,
                w: 4,
                h: 1,
            },
            Cells {
                col: 0,
                row: 1,
                w: 1,
                h: 1,
            },
        ];
        assert_eq!(
            free_place(&g, &taken, 1, 1),
            Some(Cells {
                col: 1,
                row: 1,
                w: 1,
                h: 1
            })
        );
        assert_eq!(
            free_place(&g, &taken, 4, 1),
            Some(Cells {
                col: 0,
                row: 2,
                w: 4,
                h: 1
            })
        );
        assert_eq!(free_place(&g, &taken, 4, 2), None);
        assert_eq!(
            g.rect(Cells {
                col: 1,
                row: 1,
                w: 2,
                h: 1
            }),
            Rect::new(105, 105, 190, 90)
        );
        assert_eq!(g.cell_at(250, 150), Some((2, 1)));
        assert_eq!(g.cell_at(450, 150), None);
    }
}
