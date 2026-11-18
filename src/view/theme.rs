use diff_stack::ThreeWayChange;
use rune::{Color, El, el};
use rune_kit::Kit;

use crate::palette::Palette;

pub(super) struct Provenance {
    pub(super) line: Color,
    pub(super) gutter: Color,
    pub(super) ink: Color,
    pub(super) marker: &'static str,
    pub(super) working: bool,
    pub(super) strike: Option<Color>,
}

impl Provenance {
    pub(super) fn new(change: ThreeWayChange, kit: &Kit, palette: Palette) -> Self {
        use ThreeWayChange::{
            CommittedAdd, CommittedDelete, CommittedThenRemoved, Context, UncommittedAdd,
            UncommittedDelete,
        };
        let c = &kit.c;
        let (status, amount, marker, working) = match change {
            Context => (None, 0.0, "", false),
            CommittedAdd => (Some(c.success), palette.committed_add, "+", false),
            CommittedDelete => (Some(c.danger), palette.committed_delete, "−", false),
            UncommittedAdd => (Some(c.success), palette.uncommitted_add, "+", true),
            UncommittedDelete => (Some(c.danger), palette.uncommitted_delete, "−", true),
            CommittedThenRemoved => (Some(c.success), palette.removed_tint, "+", true),
        };
        let (line, gutter, ink) = status.map_or(
            (c.bg_base, tint(c.bg_base, c.layer_01, 0.45), c.text_muted),
            |status| {
                (
                    tint(c.bg_base, status.icon, amount),
                    tint(c.bg_base, status.icon, amount * 1.3),
                    status.fg,
                )
            },
        );
        Self {
            line,
            gutter,
            ink,
            marker,
            working,
            strike: (change == CommittedThenRemoved && palette.removed_strike > 0.0)
                .then(|| c.text_muted.alpha(palette.removed_strike)),
        }
    }
}

pub(super) fn accent_surface(kit: &Kit) -> Color {
    tint(
        kit.c.bg_base,
        kit.c.info.icon,
        if kit.dark { 0.25 } else { 0.11 },
    )
}

/// Mix palette colours perceptually so the same tint stays readable in either appearance.
pub(super) fn tint(base: Color, ink: Color, amount: f32) -> Color {
    let [l, a, b, alpha] = base.to_oklab();
    let [ink_l, ink_a, ink_b, _] = ink.to_oklab();
    let mix = |from, to| from + (to - from) * amount;
    Color::from_oklab([mix(l, ink_l), mix(a, ink_a), mix(b, ink_b), alpha])
}

pub(super) fn dot(color: Color, size: f32) -> El {
    el().size(size, size).radius(size / 2.0).fill(color)
}
