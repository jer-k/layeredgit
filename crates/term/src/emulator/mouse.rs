use ghostty_vt::{Terminal, key::Mods, mouse};

use super::{Emulator, VtError};

pub(super) struct Mouse {
    encoder: mouse::Encoder,
    event: mouse::Event,
}

impl Mouse {
    pub(super) fn new() -> Result<Self, VtError> {
        Ok(Self {
            encoder: mouse::Encoder::new()?,
            event: mouse::Event::new()?,
        })
    }
}

impl Emulator {
    /// Whether the running program has requested mouse reports.
    pub fn is_mouse_tracking(&self) -> bool {
        self.terminal.is_mouse_tracking().unwrap_or(false)
    }

    /// Encodes a mouse event at a fractional position in the active grid.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot read the grid size or encode the event.
    pub fn encode_mouse(
        &mut self,
        action: mouse::Action,
        button: Option<mouse::Button>,
        mods: Mods,
        position: mouse::Position,
    ) -> Result<Vec<u8>, VtError> {
        let size = encoder_size(&self.terminal)?;
        self.mouse
            .event
            .set_action(action)
            .set_button(button)
            .set_mods(mods)
            .set_position(mouse::Position {
                x: position.x * pixels(size.cell_width),
                y: position.y * pixels(size.cell_height),
            });
        let mut bytes = Vec::new();
        self.mouse
            .encoder
            .set_options_from_terminal(&self.terminal)
            .set_size(size)
            .set_any_button_pressed(button.is_some() && action == mouse::Action::Motion)
            .encode_to_vec(&self.mouse.event, &mut bytes)?;
        Ok(bytes)
    }
}

fn encoder_size(terminal: &Terminal) -> Result<mouse::EncoderSize, VtError> {
    let cols = u32::from(terminal.cols()?);
    let rows = u32::from(terminal.rows()?);
    let cell_width = (terminal.width_px()? / cols.max(1)).max(1);
    let cell_height = (terminal.height_px()? / rows.max(1)).max(1);
    Ok(mouse::EncoderSize {
        screen_width: cols * cell_width,
        screen_height: rows * cell_height,
        cell_width,
        cell_height,
        padding_top: 0,
        padding_bottom: 0,
        padding_right: 0,
        padding_left: 0,
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "The mouse encoder takes f32 pixel coordinates"
)]
fn pixels(value: u32) -> f32 {
    value as f32
}
