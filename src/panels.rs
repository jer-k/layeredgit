//! Preferred panel widths and the space available for them in the window.

use rune::prefs::{Prefs, PrefsError};

/// Minimum width of the native review window.
pub(crate) const MIN_WINDOW_WIDTH: f32 = 960.0;
/// Minimum width of the diff pane, including its horizontal padding.
pub(crate) const MIN_DIFF_WIDTH: f32 = 392.0;
const COLLAPSED_REPOSITORIES: f32 = 53.0;
const PREF_KEY: &str = "panel_widths";

/// The two adjustable columns in a review.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Panel {
    /// The saved repositories sidebar.
    Repositories,
    /// The changed files tree.
    Files,
}

impl Panel {
    /// The panel's slot in the width and highlight arrays.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Repositories => 0,
            Self::Files => 1,
        }
    }

    /// The initial width and double-click reset value in points.
    pub(crate) const fn default_width(self) -> f32 {
        match self {
            Self::Repositories => 218.0,
            Self::Files => 266.0,
        }
    }

    const fn bounds(self) -> (f32, f32) {
        match self {
            Self::Repositories => (184.0, 400.0),
            Self::Files => (220.0, 500.0),
        }
    }
}

/// User preferences before applying the current window's limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Widths([f32; 2]);

impl Default for Widths {
    fn default() -> Self {
        Self([
            Panel::Repositories.default_width(),
            Panel::Files.default_width(),
        ])
    }
}

impl Widths {
    /// Reads saved widths, falling back to defaults for missing or invalid values.
    pub(crate) fn load(prefs: &Prefs) -> Self {
        let mut widths = Self::default();
        if let Some([repositories, files]) = prefs.get::<[f32; 2]>(PREF_KEY) {
            widths.set(Panel::Repositories, repositories);
            widths.set(Panel::Files, files);
        }
        widths
    }

    /// Saves the preferred widths through the caller's preference store.
    pub(crate) fn save(self, prefs: &mut Prefs) -> Result<(), PrefsError> {
        prefs.set(PREF_KEY, &self.0)
    }

    /// Rounds and clamps a new preference within the panel's intrinsic bounds.
    pub(crate) fn set(&mut self, panel: Panel, width: f32) {
        if width.is_finite() {
            let (min, max) = panel.bounds();
            self.0[panel.index()] = width.round().clamp(min, max);
        }
    }
}

/// The bounds each handle can use without moving the panel next to it.
#[derive(Clone, Copy)]
pub(crate) struct PanelRoom {
    min: f32,
    max: f32,
}

impl PanelRoom {
    /// Computes one panel's limits for the current window and sidebar visibility.
    pub(crate) fn new(panel: Panel, window: f32, repositories_collapsed: Option<bool>) -> Self {
        let (min, max) = panel.bounds();
        let (repos_min, _) = Panel::Repositories.bounds();
        let (files_min, _) = Panel::Files.bounds();
        let spare = (window - MIN_DIFF_WIDTH - repos_min - files_min).max(0.0);
        // Give the Files tree the larger share of spare space. Both limits are
        // independent, so moving one divider keeps the adjacent panel's width.
        let available = match (panel, repositories_collapsed) {
            (Panel::Repositories, Some(false)) => repos_min + spare * 0.45,
            (Panel::Files, Some(false)) => files_min + spare * 0.55,
            (Panel::Files, Some(true)) => window - MIN_DIFF_WIDTH - COLLAPSED_REPOSITORIES,
            _ => window - MIN_DIFF_WIDTH,
        };
        Self {
            min,
            max: max.min(available.floor()).max(min),
        }
    }

    /// Rounds an on-screen width into this window's bounds.
    pub(crate) fn clamp(self, width: f32) -> f32 {
        width.round().clamp(self.min, self.max)
    }

    /// Returns the range exposed by the handle.
    pub(crate) fn limits(self) -> (f32, f32) {
        (self.min, self.max)
    }

    /// Returns the displayed width without modifying the preference.
    pub(crate) fn width(self, panel: Panel, preferred: Widths, collapsed: bool) -> f32 {
        if panel == Panel::Repositories && collapsed {
            COLLAPSED_REPOSITORIES
        } else {
            self.clamp(preferred.0[panel.index()])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_window_caps_both_panels_and_a_larger_one_restores_the_preferences() {
        let mut preferred = Widths::default();
        preferred.set(Panel::Repositories, 700.0);
        preferred.set(Panel::Files, 500.0);
        let visible = |panel, window, collapsed| {
            PanelRoom::new(panel, window, Some(collapsed)).width(panel, preferred, collapsed)
        };
        let occupied = |window, collapsed| {
            visible(Panel::Repositories, window, collapsed)
                + visible(Panel::Files, window, collapsed)
        };
        assert!(MIN_WINDOW_WIDTH - occupied(MIN_WINDOW_WIDTH, false) >= MIN_DIFF_WIDTH);
        assert!(MIN_WINDOW_WIDTH - occupied(MIN_WINDOW_WIDTH, true) >= MIN_DIFF_WIDTH);
        assert_eq!(
            [
                visible(Panel::Repositories, 1400.0, false),
                visible(Panel::Files, 1400.0, false)
            ],
            [400.0, 500.0],
        );
    }
}
