//! Projects byte ranges onto Rune's code layout and updates only their paint.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

use parley::{
    Layout,
    editing::{Cursor, Selection},
    layout::Affinity,
};
use rune::{
    Color, El, Live, Pixels, Span, TextStyle, el,
    text::{Ink, RichLayoutOptions, TextSystem},
};

use crate::{desktop::FONT_FAMILIES, model::Line};

thread_local! {
    static TEXT: RefCell<Option<TextSystem>> = const { RefCell::new(None) };
}

/// A mounted row's hit box and selection paint. It belongs to the virtual row's memo.
pub(super) struct LinePaint {
    pub(super) drawn: Live<[f32; 4]>,
    width: Cell<f32>,
    style: TextStyle,
    scale: f32,
    wrap: bool,
    spans: Vec<Span>,
    color: Color,
    layout: RefCell<Option<Rc<Layout<Ink>>>>,
    painted: RefCell<Option<Range<usize>>>,
    single: Live<[f32; 4]>,
    mask_rect: Live<[f32; 4]>,
    mask: Pixels,
}

impl LinePaint {
    pub(super) fn new(
        style: TextStyle,
        scale: f32,
        wrap: bool,
        spans: Vec<Span>,
        color: Color,
    ) -> Self {
        Self {
            drawn: Live::new([0.0; 4]),
            width: Cell::new(0.0),
            style,
            scale,
            wrap,
            spans,
            color,
            layout: RefCell::default(),
            painted: RefCell::default(),
            single: Live::new([0.0; 4]),
            mask_rect: Live::new([0.0; 4]),
            mask: Pixels::new(),
        }
    }

    pub(super) fn elements(&self) -> [El; 2] {
        [
            el().key("selection")
                .absolute(0.0, 0.0)
                .fill(self.color)
                .live_rect(&self.single)
                .a11y_hidden(),
            el().key("selection-mask")
                .absolute(0.0, 0.0)
                .size(0.0, 0.0)
                .pixels(&self.mask)
                .live_rect(&self.mask_rect)
                .a11y_hidden(),
        ]
    }

    pub(super) fn width(&self, width: f32) {
        if (self.width.replace(width) - width).abs() > f32::EPSILON {
            if self.wrap {
                self.layout.take();
            }
            self.clear();
        }
    }

    fn layout(&self, line: &Line) -> Rc<Layout<Ink>> {
        self.layout
            .borrow_mut()
            .get_or_insert_with(|| {
                TEXT.with_borrow_mut(|text| {
                    let text = text.get_or_insert_with(|| TextSystem::new(&[], FONT_FAMILIES));
                    let options = RichLayoutOptions::default();
                    let (options, width) = if self.wrap {
                        (options.wrap_anywhere(), Some(self.width.get() * self.scale))
                    } else {
                        (options, None)
                    };
                    let layout = text.layout_rich_with(
                        &line.text,
                        &self.spans,
                        self.style,
                        self.scale,
                        width,
                        options,
                    );
                    text.end_commit();
                    layout
                })
            })
            .clone()
    }

    fn origin_x(&self, layout: &Layout<Ink>) -> f32 {
        // Rune aligns an unwrapped right-to-left paragraph to the code box's right edge.
        if !self.wrap && layout.is_rtl() {
            self.width.get() * self.scale - (layout.full_width() * 64.0).ceil() / 64.0
        } else {
            0.0
        }
    }

    pub(super) fn hit(&self, line: &Line, [x, y]: [f32; 2], word: bool) -> Range<usize> {
        let [left, top, _, height] = self.drawn.get();
        let layout = self.layout(line);
        let x = (x - left) * self.scale - self.origin_x(&layout);
        // Dragging through a hunk or a fold can land beyond this row's box.
        let y = (y - top).clamp(0.0, (height - 0.5).max(0.0)) * self.scale;
        let range = if word {
            Selection::word_from_point(&layout, x, y).text_range()
        } else {
            let byte = Cursor::from_point(&layout, x, y).index();
            byte..byte
        };
        line.snap_byte(range.start)..line.snap_byte(range.end)
    }

    pub(super) fn clear(&self) {
        self.painted.take();
        self.single.set([0.0; 4]);
        self.mask_rect.set([0.0; 4]);
    }

    pub(super) fn paint(&self, line: &Line, range: Option<Range<usize>>) {
        let range = range.filter(|range| !range.is_empty() || line.text.is_empty());
        if *self.painted.borrow() == range {
            return;
        }
        self.painted.replace(range.clone());
        let Some(range) = range else {
            self.clear();
            return;
        };
        let layout = self.layout(line);
        let x = f64::from(self.origin_x(&layout));
        let scale = f64::from(self.scale);
        let boxes = Selection::new(
            Cursor::from_byte_index(&layout, range.start, Affinity::Downstream),
            Cursor::from_byte_index(&layout, range.end, Affinity::Upstream),
        )
        .geometry(&layout);
        #[allow(
            clippy::cast_possible_truncation,
            reason = "Rune paints in f32 layout points"
        )]
        let rects: Vec<_> = boxes
            .into_iter()
            .map(|(b, _)| {
                [
                    (x + b.x0) / scale,
                    b.y0 / scale,
                    (x + b.x1) / scale,
                    b.y1 / scale,
                ]
                .map(|n| n as f32)
            })
            .collect();
        match rects.as_slice() {
            [] if line.text.is_empty() => {
                self.single
                    .set([0.0, 0.0, self.style.size / 2.0, self.style.line_height]);
                self.mask_rect.set([0.0; 4]);
            }
            [] => self.clear(),
            &[[left, top, right, bottom]] => {
                self.single.set([left, top, right - left, bottom - top]);
                self.mask_rect.set([0.0; 4]);
            }
            _ => {
                self.single.set([0.0; 4]);
                self.paint_mask(&rects);
            }
        }
    }

    /// A wrapped or bidi selection can produce any number of boxes. Stream one bounded mask
    /// in that case. Ordinary lines use a live rectangle and never upload an image.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "The mask uses clamped pixel coordinates and colour channels"
    )]
    fn paint_mask(&self, rects: &[[f32; 4]]) {
        let [left, top, right, bottom] = rects.iter().copied().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |[x0, y0, x1, y1], [left, top, right, bottom]| {
                [x0.min(left), y0.min(top), x1.max(right), y1.max(bottom)]
            },
        );
        let (w, h) = (right - left, bottom - top);
        if w <= 0.0 || h <= 0.0 {
            self.mask_rect.set([0.0; 4]);
            return;
        }
        let x_scale = self.scale.min(1024.0 / w);
        let y_scale = self.scale.min(1024.0 / h);
        let width = (w * x_scale).ceil() as u32;
        let height = (h * y_scale).ceil() as u32;
        let stride = width as usize * 4;
        let mut pixels = vec![0; stride * height as usize];
        let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        let [red, green, blue] = [self.color.r, self.color.g, self.color.b]
            .map(|c| channel(rune::color::srgb_encode(c)));
        let ink = [red, green, blue, channel(self.color.a)];
        for &[x0, y0, x1, y1] in rects {
            let (x0, x1) = (
                ((x0 - left) * x_scale).round() as usize,
                ((x1 - left) * x_scale).round() as usize,
            );
            let (y0, y1) = (
                ((y0 - top) * y_scale).round() as usize,
                ((y1 - top) * y_scale).round() as usize,
            );
            for row in pixels.chunks_exact_mut(stride).take(y1).skip(y0) {
                if let Some(span) = row.get_mut(x0 * 4..x1 * 4) {
                    for pixel in span.as_chunks_mut::<4>().0 {
                        *pixel = ink;
                    }
                }
            }
        }
        self.mask.set(width, height, pixels);
        self.mask_rect.set([left, top, w, h]);
    }
}
