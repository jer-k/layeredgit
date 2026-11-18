//! Direct manipulation of the side panels and saving completed changes.

use std::{cell::Cell, rc::Rc};

use rune::{Drag, Live, Phase, Tx};

use crate::{
    panels::{Panel, PanelRoom, Widths},
    repositories::Request,
};

use super::State;

/// The current drag preview and committed widths for the review window.
pub(crate) struct Panels {
    /// Committed widths shared with the isolated headless runner.
    pub(crate) preferred: Rc<Cell<Widths>>,
    drag: Option<(Panel, f32)>,
    highlight: [Live<f32>; 2],
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            preferred: Rc::default(),
            drag: None,
            highlight: [Live::new(0.0), Live::new(0.0)],
        }
    }
}

impl Panels {
    /// The panel's rendered width, following the pointer while it is dragged.
    pub(crate) fn width(&self, panel: Panel, room: PanelRoom, collapsed: bool) -> f32 {
        if let Some((active, width)) = self.drag
            && active == panel
            && !collapsed
        {
            return room.clamp(width);
        }
        room.width(panel, self.preferred.get(), collapsed)
    }

    /// The live drag highlight for the handle.
    pub(crate) fn highlight(&self, panel: Panel) -> &Live<f32> {
        &self.highlight[panel.index()]
    }

    pub(super) fn cancel_drag(&mut self) {
        self.drag = None;
        for highlight in &self.highlight {
            highlight.set(0.0);
        }
    }
}

impl State {
    /// Previews a width during a gesture and commits it on release.
    pub(crate) fn drag_panel(
        &mut self,
        panel: Panel,
        room: PanelRoom,
        drag: &mut Drag,
        tx: &mut Tx,
    ) {
        match drag.phase {
            Phase::Down => {
                let stale = self.panels.drag.is_some();
                self.panels.cancel_drag();
                drag.data = self.panels.width(panel, room, false);
                self.panels.drag = Some((panel, drag.data));
                self.panels.highlight(panel).set(1.0);
                if stale {
                    tx.instant();
                } else {
                    tx.live_only();
                }
            }
            Phase::Move => {
                let Some((_, width)) = self
                    .panels
                    .drag
                    .as_mut()
                    .filter(|(active, _)| *active == panel)
                else {
                    tx.live_only();
                    return;
                };
                let next = room.clamp(drag.data + drag.dx);
                let changed = (room.clamp(*width) - next).abs() >= 0.5;
                *width = next;
                if changed {
                    tx.instant();
                } else {
                    // Extra travel at a limit does not change the layout.
                    tx.live_only();
                }
            }
            Phase::Up => {
                let Some((_, width)) = self.panels.drag.filter(|(active, _)| *active == panel)
                else {
                    tx.live_only();
                    return;
                };
                self.panels.cancel_drag();
                let width = room.clamp(width);
                if !drag.cancelled() && (room.clamp(drag.data) - width).abs() >= 0.5 {
                    self.resize_panel(panel, width, tx);
                } else if (self.panels.width(panel, room, false) - width).abs() >= 0.5 {
                    tx.instant();
                } else {
                    tx.live_only();
                }
            }
        }
    }

    /// Applies a preferred width and queues its save on the repository worker.
    pub(crate) fn resize_panel(&mut self, panel: Panel, width: f32, tx: &mut Tx) {
        let previous = self.panels.preferred.get();
        let mut widths = previous;
        widths.set(panel, width);
        if previous != widths {
            self.panels.preferred.set(widths);
            if let Some(workspace) = &self.workspace {
                workspace.send(Request::ResizePanels(widths));
            }
        }
        tx.instant();
    }
}
