use rune::{El, Live, el, row, text};
use rune_kit::Kit;

use crate::{palette::Palette, state::State};

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
    let label = match (above, below) {
        (0, 0) => return None,
        (0, count) => format!("{} changed below ↓", lines(count)),
        (count, 0) => format!("{} changed above ↑", lines(count)),
        (above, below) => format!("Changed ↑ {above} · ↓ {below}"),
    };
    Some(
        row()
            .key("pulse-offscreen")
            .role("status")
            .pin(None, None, Some(12.0), Some(12.0))
            .px(10.0)
            .py(5.0)
            .radius(kit.radius.md)
            .fill(tint(
                kit.c.bg_base,
                palette.pulse(),
                if kit.dark { 0.24 } else { 0.13 },
            ))
            .border(1.0, tint(kit.c.bg_base, palette.pulse(), 0.32))
            .live_opacity(state.pulse.opacity())
            .child(
                text(label)
                    .font(kit.ty.text_xsmall_medium)
                    .color(kit.c.text_base),
            ),
    )
}

fn lines(count: usize) -> String {
    if count == 1 {
        "1 line".into()
    } else {
        format!("{count} lines")
    }
}
