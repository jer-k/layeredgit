use std::ops::Range;

/// Ghostty's viewport spans the entire PTY grid, which may be taller than the pane.
pub(super) struct Grid {
    pub(super) rows: usize,
    pub(super) offset: usize,
    pub(super) history: usize,
    pub(super) cursor: usize,
}

#[derive(Default)]
enum Position {
    #[default]
    Following,
    /// Ghostty already pins the first row in scrollback.
    History,
    /// An absolute row in the live grid that Ghostty cannot pin until it enters history.
    Active(usize),
}

#[derive(Default)]
pub(super) struct Viewport {
    position: Position,
    pub(super) first: usize,
}

impl Viewport {
    pub(super) fn follow(&mut self) {
        self.position = Position::Following;
    }

    pub(super) fn visible(&self, screen_rows: usize, pane_rows: usize) -> Range<usize> {
        let start = self.first.min(screen_rows);
        start..start.saturating_add(pane_rows).min(screen_rows)
    }

    /// Updates the slice within Ghostty's viewport. Returns a row to pin when output
    /// pushes the row being browsed out of the live grid and into scrollback.
    pub(super) fn layout(&mut self, grid: &Grid, pane_rows: usize) -> Option<usize> {
        let max_first = grid.rows.saturating_sub(pane_rows);
        self.first = match self.position {
            Position::Following => follow_first(grid, pane_rows),
            Position::History => 0,
            Position::Active(row) => {
                if let Some(first) = row.checked_sub(grid.history) {
                    first.min(max_first)
                } else {
                    self.position = Position::History;
                    self.first = 0;
                    return Some(row);
                }
            }
        };
        None
    }

    /// Moves the pane by rows. Returns the new offset to give Ghostty when scrolling.
    pub(super) fn scroll(&mut self, grid: &Grid, pane_rows: usize, delta: isize) -> Option<usize> {
        let current = grid.offset + self.first;
        let next = current.saturating_add_signed(delta);
        if delta > 0 && next >= grid.history + follow_first(grid, pane_rows) {
            self.follow();
            return (grid.offset != grid.history).then_some(grid.history);
        }
        if next == current {
            return None;
        }
        if next < grid.history {
            self.position = Position::History;
            Some(next)
        } else {
            self.position = Position::Active(next);
            (grid.offset != grid.history).then_some(grid.history)
        }
    }
}

fn follow_first(grid: &Grid, pane_rows: usize) -> usize {
    grid.cursor
        .saturating_sub(pane_rows.saturating_sub(1))
        .min(grid.rows.saturating_sub(pane_rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_cursor_in_a_larger_grid() {
        let mut viewport = Viewport::default();
        let mut grid = Grid {
            rows: 53,
            offset: 0,
            history: 0,
            cursor: 52,
        };
        assert_eq!(viewport.layout(&grid, 11), None);
        assert_eq!(viewport.visible(53, 11), 42..53);
        grid.cursor = 6;
        viewport.layout(&grid, 11);
        assert_eq!(viewport.visible(53, 11), 0..11);
        grid.cursor = 12;
        viewport.layout(&grid, 11);
        assert_eq!(viewport.visible(53, 11), 2..13);
        viewport.layout(&grid, 53);
        assert_eq!(viewport.visible(53, 53), 0..53);
    }

    #[test]
    fn browses_live_rows_then_pins_history_until_following_resumes() {
        let mut viewport = Viewport::default();
        let mut grid = Grid {
            rows: 53,
            offset: 100,
            history: 100,
            cursor: 52,
        };
        viewport.layout(&grid, 11);
        assert_eq!(viewport.scroll(&grid, 11, -10), None);
        viewport.layout(&grid, 11);
        assert_eq!(viewport.first, 32);

        grid.history = 135;
        grid.offset = 135;
        assert_eq!(viewport.layout(&grid, 11), Some(132));
        assert_eq!(viewport.first, 0);
        grid.offset = 132;
        grid.history = 150;
        assert_eq!(viewport.layout(&grid, 11), None);
        assert_eq!(viewport.scroll(&grid, 11, -2), Some(130));
        grid.offset = 130;
        assert_eq!(viewport.scroll(&grid, 11, 22), Some(150));
        grid.offset = 150;
        viewport.layout(&grid, 11);
        assert_eq!(viewport.first, 2);

        viewport.follow();
        viewport.layout(&grid, 11);
        assert_eq!(viewport.first, 42);
        assert_eq!(viewport.scroll(&grid, 11, -50), Some(142));
        grid.offset = 142;
        viewport.layout(&grid, 11);
        assert_eq!(viewport.scroll(&grid, 11, 50), Some(150));
        grid.offset = 150;
        viewport.layout(&grid, 11);
        assert_eq!(viewport.first, 42);
    }
}
