use rune::{Cursor, El, Live, Transition, el, row, text};
use rune_kit::Kit;

use crate::{
    palette::Palette,
    state::{PulseDirection, State},
};

use super::theme::tint;

/// Paints below the text and uses Rune's live opacity without rebuilding the row.
pub(super) fn wash(kit: &Kit, palette: Palette, opacity: &Live<f32>, radius: f32) -> El {
    el().key("pulse-tint")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .radius(radius)
        .fill(palette.pulse().alpha(if kit.dark { 0.25 } else { 0.20 }))
        .live_opacity(opacity)
        .a11y_hidden()
}

pub(super) fn hint(state: &State, kit: &Kit, palette: Palette) -> Option<El> {
    let (above, below) = state.pulse_offscreen();
    if above == 0 && below == 0 {
        return None;
    }
    Some(
        row()
            .key("pulse-offscreen")
            .pin(None, None, Some(12.0), Some(12.0))
            .gap(6.0)
            .instant_changes()
            .transition(Transition::none())
            .live_opacity(state.pulse.opacity())
            .children(
                [
                    (PulseDirection::Above, above),
                    (PulseDirection::Below, below),
                ]
                .into_iter()
                .filter(|&(_, count)| count > 0)
                .map(|(direction, count)| jump_button(count, direction, kit, palette)),
            ),
    )
}

fn jump_button(count: usize, direction: PulseDirection, kit: &Kit, palette: Palette) -> El {
    let (key, position, arrow, shortcut) = match direction {
        PulseDirection::Above => ("above", "above", "↑", "p"),
        PulseDirection::Below => ("below", "below", "↓", "n"),
    };
    let label = format!("{} changed {position}", lines(count));
    row()
        .key(key)
        .role("button")
        .label(format!("{label}. Jump to nearest change"))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .help(format!("Jump to nearest change {position} ({shortcut})"))
        .on_press_tx(move |state: &mut State, tx| {
            state.jump_to_pulse(direction, tx);
        })
        .px(10.0)
        .py(5.0)
        .radius(kit.radius.md)
        .fill(tint(
            kit.c.bg_base,
            palette.pulse(),
            if kit.dark { 0.24 } else { 0.13 },
        ))
        .border(1.0, tint(kit.c.bg_base, palette.pulse(), 0.32))
        .hover(kit.c.hover)
        .press(kit.c.pressed)
        .child(
            text(format!("{label} {arrow}"))
                .font(kit.ty.text_xsmall_medium)
                .color(kit.c.text_base),
        )
}

fn lines(count: usize) -> String {
    if count == 1 {
        "1 line".into()
    } else {
        format!("{count} lines")
    }
}
