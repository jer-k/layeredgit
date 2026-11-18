//! A keyboard-accessible resize target inset into each panel's trailing edge.

use rune::{Cursor, El, Transition, el};
use rune_kit::Kit;

use crate::{
    panels::{Panel, PanelRoom},
    state::State,
};

use super::theme::tint;

const STEP: f32 = 8.0;

pub(super) fn handle(panel: Panel, room: PanelRoom, state: &State, kit: &Kit) -> El {
    let (key, label) = match panel {
        Panel::Repositories => ("resize-repositories", "Resize Repositories panel"),
        Panel::Files => ("resize-files", "Resize Files changed panel"),
    };
    let width = state.panels.width(panel, room, false);
    let (min, max) = room.limits();
    let highlight = tint(kit.c.bg_base, kit.c.info.icon, 0.65);
    let line = || {
        el().pin(None, Some(0.0), Some(0.0), Some(0.0))
            .w(2.0)
            .fill(highlight)
            .a11y_hidden()
    };
    el().key(key)
        .role("separator")
        .label(label)
        .range_value(f64::from(width), f64::from(min), f64::from(max))
        .range_step(f64::from(STEP))
        .pin(None, Some(0.0), Some(0.0), Some(0.0))
        .w(8.0)
        .cursor(Cursor::ResizeColumn)
        .transition(Transition::none())
        .hover_group()
        .focus_ring(1.0, -1.0, highlight)
        .on_step(move |state: &mut State, steps, tx| {
            let width = state.panels.width(panel, room, false);
            // A large accessibility step will reach a limit well before i16 overflows.
            let steps = i16::try_from(steps).unwrap_or(if steps < 0 { i16::MIN } else { i16::MAX });
            let next = room.clamp(width + STEP * f32::from(steps));
            if (next - width).abs() >= 0.5 {
                state.resize_panel(panel, next, tx);
            } else {
                tx.live_only();
            }
        })
        .on_drag(move |state: &mut State, drag, tx| state.drag_panel(panel, room, drag, tx))
        .on_double_press_tx(move |state: &mut State, tx| {
            state.resize_panel(panel, panel.default_width(), tx);
        })
        .child(line().group_opacity(0.0, 1.0))
        .child(line().live_opacity(state.panels.highlight(panel)))
}
