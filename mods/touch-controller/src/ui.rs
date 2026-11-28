//! Drawing the controls on the HUD, in GUI pixels.
//!
//! Nothing here reads input: it paints the layout the touch hook resolved, so
//! what a finger sees and what it hits are the same rectangle.
use crate::layout::{BUTTONS, Button, Layout, Mode};
use mod_api::DrawContext;

/// What this frame draws: the controls' state, without the mod's own types.
pub struct View<'a> {
    pub stick: [f32; 2],
    pub held: &'a [bool; BUTTONS.len()],
    pub stick_active: bool,
    pub opacity: f32,
    /// The label for one control, already translated.
    pub label: &'a dyn Fn(Button) -> String,
}

/// Paints the stick and every button, then their labels.
pub fn draw(batch: &mut dyn DrawContext, layout: &Layout, view: &View<'_>) {
    let [width, height] = batch.size();
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let aspect = width / height;
    let alpha = |base: u32| -> u32 {
        let a = ((base >> 24) & 0xff) as f32 * view.opacity.clamp(0.0, 1.0);
        (a as u32) << 24 | (base & 0x00ff_ffff)
    };

    // The pad first, so a button drawn over it stays legible.
    let stick = layout.stick.rect(aspect);
    let (sx, sy, sw) = (stick[0] * width, stick[1] * height, stick[2] * width);
    let radius = (sw / 2.0).max(1.0);
    disc(batch, sx + radius, sy + radius, radius, alpha(0x40000000));
    disc(
        batch,
        sx + radius,
        sy + radius,
        radius * 0.94,
        alpha(0x14FFFFFF),
    );
    if layout.mode == Mode::Joystick {
        let knob = radius * 0.40;
        let cx = sx + radius + view.stick[1] * radius * 0.55;
        let cy = sy + radius - view.stick[0] * radius * 0.55;
        disc(
            batch,
            cx,
            cy,
            knob,
            alpha(if view.stick_active {
                0x99FFFFFF
            } else {
                0x66FFFFFF
            }),
        );
    } else {
        // A D-pad: four notches round the same circle, lit while they are held.
        for (index, direction) in [[0.0, -1.0], [0.0, 1.0], [-1.0, 0.0], [1.0, 0.0]]
            .into_iter()
            .enumerate()
        {
            let lit = match index {
                0 => view.stick[0] > 0.0,
                1 => view.stick[0] < 0.0,
                2 => view.stick[1] < 0.0,
                _ => view.stick[1] > 0.0,
            };
            let size = radius * 0.52;
            let cx = sx + radius + direction[0] * radius * 0.48;
            let cy = sy + radius + direction[1] * radius * 0.48;
            let color = if lit { 0x99FFFFFF } else { 0x44FFFFFF };
            batch.fill([cx - size / 2.0, cy - size / 2.0, size, size], alpha(color));
        }
    }

    for button in BUTTONS {
        let rect = layout.buttons[button.index()].rect(aspect);
        let x = rect[0] * width;
        let y = rect[1] * height;
        let size = rect[3] * height;
        let held = view.held[button.index()];
        let panel = if held { 0x55FFFFFF } else { 0x0FFFFFFF };
        // Two fills: a dark rim and a lit face inside it.
        batch.fill([x, y, size, size], alpha(0x59000000));
        batch.fill([x + 1.5, y + 1.5, size - 3.0, size - 3.0], alpha(panel));
        if held {
            batch.fill([x + 1.5, y + 1.5, size - 3.0, 1.5], alpha(0xAAFFFFFF));
        }
        let text = (view.label)(button);
        let text_width = text_width(&text);
        let baseline = size * 0.5 - 4.0;
        batch.text(
            &text,
            [x + (size - text_width) / 2.0, y + baseline],
            0xFFFFFFFF,
            true,
        );
    }
}

/// A filled circle, as horizontal strips: the GUI batch draws rectangles only.
fn disc(batch: &mut dyn DrawContext, cx: f32, cy: f32, radius: f32, argb: u32) {
    const STRIPS: usize = 16;
    if radius <= 0.0 {
        return;
    }
    let step = 2.0 * radius / STRIPS as f32;
    for strip in 0..STRIPS {
        let t = -1.0 + (strip as f32 + 0.5) * 2.0 / STRIPS as f32;
        let half = radius * (1.0 - t * t).max(0.0).sqrt();
        if half <= 0.0 {
            continue;
        }
        batch.fill(
            [
                cx - half,
                cy + radius * t - step * 0.5,
                half * 2.0,
                step + 0.5,
            ],
            argb,
        );
    }
}

/// How wide a label draws, near enough to centre it: the GUI font is about
/// six pixels a character at scale one, wide glyphs about nine.
fn text_width(text: &str) -> f32 {
    text.chars()
        .map(|c| if c.is_ascii() { 6.0 } else { 9.0 })
        .sum::<f32>()
}
