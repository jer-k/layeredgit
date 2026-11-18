//! Review palette defaults and the optional live tuning panel.

use rune::{Color, oklch};

#[cfg(feature = "dev")]
const PAGE: &str = "Diff palette";
#[cfg(feature = "dev")]
const LIGHT_GROUP: &str = "Light provenance";
#[cfg(feature = "dev")]
const DARK_GROUP: &str = "Dark provenance";

// Keep the defaults beside the knob ranges so shipped and dev builds start
// with the same values. Each row ends with the light and dark defaults.
macro_rules! palette {
    ($($field:ident: $label:literal, $min:literal, $max:literal, $step:literal, $light:literal, $dark:literal;)*) => {
        #[derive(Clone, Copy)]
        pub(crate) struct Palette {
            $(pub(crate) $field: f32,)*
        }

        impl Palette {
            pub(crate) const fn defaults(dark: bool) -> Self {
                Self { $($field: if dark { $dark } else { $light },)* }
            }

            pub(crate) fn memo_key(self) -> impl std::hash::Hash {
                [$(self.$field,)*].map(f32::to_bits)
            }
        }

        #[cfg(feature = "dev")]
        mod panel {
            use rune_knobs::{Group, Knob, Knobs};
            use super::{DARK_GROUP, LIGHT_GROUP, PAGE, Palette};

            const LIGHT: &[Knob] = &[
                $(Knob::new(concat!("Light ", $label), $min, $max, $light).label($label).step($step),)*
            ];
            const DARK: &[Knob] = &[
                $(Knob::new(concat!("Dark ", $label), $min, $max, $dark).label($label).step($step),)*
            ];

            pub(super) const GROUPS: &[Group] = &[
                Group::new(LIGHT_GROUP, "diff", LIGHT).under(PAGE),
                Group::new(DARK_GROUP, "diff", DARK).under(PAGE),
            ];

            pub(super) fn values(knobs: &Knobs, dark: bool) -> Palette {
                let [$($field,)*] = (if dark { DARK } else { LIGHT }) else {
                    return Palette::defaults(dark);
                };
                Palette { $($field: knobs.get($field),)* }
            }
        }
    }
}

palette! {
    committed_add: "Committed add tint", 0.0, 0.45, 0.01, 0.15, 0.20;
    committed_delete: "Committed delete tint", 0.0, 0.45, 0.01, 0.15, 0.20;
    uncommitted_add: "Working add tint", 0.0, 0.45, 0.01, 0.15, 0.20;
    uncommitted_delete: "Working delete tint", 0.0, 0.45, 0.01, 0.15, 0.20;
    edge_width: "Working edge width", 0.0, 8.0, 0.5, 3.0, 3.0;
    edge_lightness: "Edge lightness", 0.0, 1.0, 0.01, 0.50, 0.75;
    edge_chroma: "Edge chroma", 0.0, 0.30, 0.01, 0.15, 0.15;
    edge_hue: "Edge hue", 0.0, 360.0, 1.0, 256.0, 256.0;
    dot_lightness: "Dot lightness", 0.0, 1.0, 0.01, 0.50, 0.75;
    dot_chroma: "Dot chroma", 0.0, 0.30, 0.01, 0.15, 0.15;
    dot_hue: "Dot hue", 0.0, 360.0, 1.0, 256.0, 256.0;
    removed_tint: "Added then removed tint", 0.0, 0.45, 0.01, 0.15, 0.20;
    removed_strike: "Strikethrough opacity", 0.0, 1.0, 0.05, 0.70, 0.70;
    pulse_lightness: "Pulse lightness", 0.0, 1.0, 0.01, 0.65, 0.84;
    pulse_chroma: "Pulse chroma", 0.0, 0.30, 0.01, 0.15, 0.14;
    pulse_hue: "Pulse hue", 0.0, 360.0, 1.0, 62.0, 75.0;
    pulse_seconds: "Pulse seconds", 0.2, 3.0, 0.05, 1.2, 1.2;
}

impl Palette {
    pub(crate) fn edge(self) -> Color {
        oklch(self.edge_lightness, self.edge_chroma, self.edge_hue)
    }

    pub(crate) fn dot(self) -> Color {
        oklch(self.dot_lightness, self.dot_chroma, self.dot_hue)
    }

    pub(crate) fn pulse(self) -> Color {
        oklch(self.pulse_lightness, self.pulse_chroma, self.pulse_hue)
    }
}

#[cfg(feature = "dev")]
#[derive(Default)]
pub(crate) struct Tuning {
    knobs: rune_knobs::Knobs,
}

#[cfg(feature = "dev")]
impl Tuning {
    pub(crate) fn palette(&self, dark: bool) -> Palette {
        panel::values(&self.knobs, dark)
    }

    pub(crate) fn key(&mut self, event: &rune::KeyEvent, dark: bool, tx: &mut rune::Tx) -> bool {
        if rune_knobs::key(&mut self.knobs, event) {
            if self.knobs.open {
                let (active, other) = if dark {
                    (DARK_GROUP, LIGHT_GROUP)
                } else {
                    (LIGHT_GROUP, DARK_GROUP)
                };
                self.knobs.pick(active);
                self.knobs.set_folded(PAGE, false);
                self.knobs.set_folded(other, true);
            }
            tx.motion(self.knobs.slide());
            return true;
        }
        false
    }

    pub(crate) fn panel(&self, content: rune::El, dark: bool) -> rune::El {
        let style = if dark {
            rune_knobs::Style::dark()
        } else {
            rune_knobs::Style::light()
        };
        rune::col().grow(1.0).min_h(0.0).child(rune_knobs::side(
            content,
            &self.knobs,
            panel::GROUPS,
            style,
            |state: &mut crate::state::State| &mut state.tuning.knobs,
        ))
    }
}
