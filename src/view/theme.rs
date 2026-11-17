use diff_stack::ThreeWayChange;
use rune::{Color, El, el};
use rune_kit::Kit;

pub(super) struct Provenance {
    pub(super) line: Color,
    pub(super) gutter: Color,
    pub(super) ink: Color,
    pub(super) marker: &'static str,
    pub(super) working: bool,
    pub(super) struck: bool,
}

impl Provenance {
    pub(super) fn new(change: ThreeWayChange, kit: &Kit) -> Self {
        use ThreeWayChange::{
            CommittedAdd, CommittedDelete, CommittedThenRemoved, Context, UncommittedAdd,
            UncommittedDelete,
        };
        let c = &kit.c;
        let (status, marker, working, struck) = match change {
            Context => (None, "", false, false),
            CommittedAdd => (Some(c.success), "+", false, false),
            CommittedDelete => (Some(c.danger), "−", false, false),
            UncommittedAdd => (Some(c.success), "+", true, false),
            UncommittedDelete => (Some(c.danger), "−", true, false),
            CommittedThenRemoved => (Some(c.success), "+", true, true),
        };
        let (line, gutter, ink) = status.map_or(
            (c.bg_base, tint(c.bg_base, c.layer_01, 0.45), c.text_muted),
            |status| {
                let line = if kit.dark { 0.20 } else { 0.15 };
                (
                    tint(c.bg_base, status.icon, line),
                    tint(c.bg_base, status.icon, line + 0.06),
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
            struck,
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

pub(super) fn dot(kit: &Kit, size: f32) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .fill(kit.c.info.icon)
}
