//! Where the controls sit and how a finger maps to them.
//!
//! A [`Place`] is a control's centre as a fraction of the surface (`cx` across
//! its width, `cy` down its height) and its size as a fraction of the surface
//! height, so a control stays the same physical size on a wide phone and on a
//! tablet. The touch input reports points the same way, which makes hit tests
//! plain arithmetic with no GUI scale in it at all: drawing converts with the
//! `DrawContext`'s own size and gets the same pixels.
use std::collections::HashMap;

/// How the player moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// A stick at `stick`: its offset is the movement, proportionally.
    #[default]
    Joystick,
    /// Four direction keys inside `stick`'s square.
    Dpad,
}

/// The controls the mod owns, drawn in this order.
pub const BUTTONS: [Button; 7] = [
    Button::Attack,
    Button::Use,
    Button::Jump,
    Button::Sneak,
    Button::Sprint,
    Button::Inventory,
    Button::Swap,
];

/// One on-screen button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Attack,
    Use,
    Jump,
    Sneak,
    Sprint,
    Inventory,
    Swap,
}

impl Button {
    /// This button's row in [`Layout::buttons`].
    pub fn index(self) -> usize {
        BUTTONS.iter().position(|b| *b == self).unwrap_or(0)
    }

    /// The key its config table is called (`[button.attack]`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Attack => "attack",
            Self::Use => "use",
            Self::Jump => "jump",
            Self::Sneak => "sneak",
            Self::Sprint => "sprint",
            Self::Inventory => "inventory",
            Self::Swap => "swap",
        }
    }

    /// The translation key of its label.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Attack => "touch_controller.button.attack",
            Self::Use => "touch_controller.button.use",
            Self::Jump => "touch_controller.button.jump",
            Self::Sneak => "touch_controller.button.sneak",
            Self::Sprint => "touch_controller.button.sprint",
            Self::Inventory => "touch_controller.button.inventory",
            Self::Swap => "touch_controller.button.swap",
        }
    }

    /// The vanilla mapping a press holds, as `ClientHost::set_mapping_down` takes it.
    pub fn mapping(self) -> &'static str {
        match self {
            Self::Attack => "key.attack",
            Self::Use => "key.use",
            Self::Jump => "key.jump",
            Self::Sneak => "key.sneak",
            Self::Sprint => "key.sprint",
            Self::Inventory => "key.inventory",
            Self::Swap => "key.swapOffhand",
        }
    }

    /// Whether it is one click rather than a mapping held while the finger is down.
    pub fn click_only(self) -> bool {
        matches!(self, Self::Inventory | Self::Swap)
    }

    /// Where it sits when the config file says nothing.
    fn default_place(self) -> Place {
        match self {
            Self::Attack => Place::new(0.855, 0.855, 0.093),
            Self::Use => Place::new(0.955, 0.690, 0.078),
            Self::Jump => Place::new(0.745, 0.735, 0.081),
            Self::Sneak => Place::new(0.090, 0.495, 0.064),
            Self::Sprint => Place::new(0.215, 0.560, 0.064),
            Self::Inventory => Place::new(0.950, 0.105, 0.058),
            Self::Swap => Place::new(0.845, 0.105, 0.058),
        }
    }
}

/// A control's centre and size on the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// Centre across the surface, `0..1` from the left.
    pub cx: f32,
    /// Centre down the surface, `0..1` from the top.
    pub cy: f32,
    /// Size as a fraction of the surface height; a button is square on screen.
    pub size: f32,
}

impl Place {
    pub const fn new(cx: f32, cy: f32, size: f32) -> Self {
        Self { cx, cy, size }
    }

    /// This control as a rectangle `[x, y, w, h]` in surface fractions, for a
    /// surface `aspect` wide (`width / height`).
    pub fn rect(self, aspect: f32) -> [f32; 4] {
        let aspect = aspect.max(0.1);
        let w = self.size / aspect;
        [self.cx - w / 2.0, self.cy - self.size / 2.0, w, self.size]
    }

    /// Brings a value read from a file back onto the screen and into range.
    fn sanitize(&mut self) {
        self.cx = finite_or(self.cx, 0.5).clamp(0.02, 0.98);
        self.cy = finite_or(self.cy, 0.5).clamp(0.02, 0.98);
        self.size = finite_or(self.size, 0.08).clamp(0.035, 0.30);
    }
}

/// The resolved layout: every control's place, after the config file and settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The stick or D-pad's square; its `size` is the diameter of the stick.
    pub stick: Place,
    pub buttons: [Place; BUTTONS.len()],
    pub mode: Mode,
}

impl Layout {
    /// The built-in layout.
    pub fn builtin() -> Self {
        Self {
            stick: Place::new(0.170, 0.700, 0.230),
            buttons: BUTTONS.map(Button::default_place),
            mode: Mode::Joystick,
        }
    }

    /// The layout a config file describes: every key it sets replaces the
    /// built-in value, and every key it leaves out keeps it. Unknown sections
    /// and keys are kept as no-ops, so a file written for a later version still
    /// loads.
    pub fn parse(text: &str) -> Self {
        let mut layout = Self::builtin();
        let values = entries(text);
        if let Some(mode) = values.get("layout.mode").and_then(|v| word(v)) {
            layout.mode = match mode.as_str() {
                "dpad" | "d-pad" | "d_pad" => Mode::Dpad,
                _ => Mode::Joystick,
            };
        }
        if let Some(place) = place_of(&values, "stick", layout.stick) {
            layout.stick = place;
        }
        for button in BUTTONS {
            let section = format!("button.{}", button.id());
            if let Some(place) = place_of(&values, &section, button.default_place()) {
                layout.buttons[button.index()] = place;
            }
        }
        layout
    }

    /// Mirrors the layout for a left-handed player: every control keeps its
    /// distance from its own edge, and the centre line stays clear.
    pub fn mirror(&mut self) {
        self.stick.cx = 1.0 - self.stick.cx;
        for place in &mut self.buttons {
            place.cx = 1.0 - place.cx;
        }
    }

    /// Grows or shrinks every control by `scale`.
    pub fn scale(&mut self, scale: f32) {
        let scale = finite_or(scale, 1.0).clamp(0.5, 2.0);
        self.stick.size *= scale;
        for place in &mut self.buttons {
            place.size *= scale;
        }
    }

    /// The button under `point`, topmost first.
    pub fn button_at(&self, point: [f32; 2], aspect: f32) -> Option<Button> {
        BUTTONS
            .iter()
            .rev()
            .copied()
            .find(|b| contains(self.buttons[b.index()], point, aspect))
    }

    /// Whether `point` is on the stick or D-pad.
    pub fn on_stick(&self, point: [f32; 2], aspect: f32) -> bool {
        let aspect = aspect.max(0.1);
        let dx = (point[0] - self.stick.cx) * aspect;
        let dy = point[1] - self.stick.cy;
        let radius = self.stick.size / 2.0 * 1.15;
        dx * dx + dy * dy <= radius * radius
    }

    /// The D-pad direction `point` asks for: forward, strafe.
    pub fn dpad(&self, point: [f32; 2], aspect: f32, deadzone: f32) -> [f32; 2] {
        let aspect = aspect.max(0.1);
        let dx = (point[0] - self.stick.cx) * aspect / (self.stick.size / 2.0).max(0.001);
        let dy = (point[1] - self.stick.cy) / (self.stick.size / 2.0).max(0.001);
        let axis = |v: f32| {
            if v.abs() < deadzone.max(0.15) {
                0.0
            } else if v > 0.0 {
                1.0
            } else {
                -1.0
            }
        };
        [axis(dy), axis(dx)]
    }

    /// The stick's movement for `point`: forward, strafe, each `-1..1` and
    /// trimmed to the unit circle, with `deadzone` ignored around the centre.
    pub fn stick_vector(&self, point: [f32; 2], aspect: f32, deadzone: f32) -> [f32; 2] {
        let aspect = aspect.max(0.1);
        let radius = (self.stick.size / 2.0).max(0.001);
        let dx = (point[0] - self.stick.cx) * aspect / radius;
        let dy = (point[1] - self.stick.cy) / radius;
        let length = (dx * dx + dy * dy).sqrt();
        if length <= deadzone {
            return [0.0, 0.0];
        }
        let scaled = ((length - deadzone) / (1.0 - deadzone)).min(1.0) / length;
        let (dx, dy) = (dx * scaled, dy * scaled);
        // Screen y grows downwards; forward is up.
        [(-dy).clamp(-1.0, 1.0), dx.clamp(-1.0, 1.0)]
    }
}

fn contains(place: Place, point: [f32; 2], aspect: f32) -> bool {
    let [x, y, w, h] = place.rect(aspect);
    let slop = 0.012;
    point[0] >= x - slop
        && point[0] <= x + w + slop
        && point[1] >= y - slop
        && point[1] <= y + h + slop
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn place_of(values: &HashMap<String, String>, section: &str, base: Place) -> Option<Place> {
    let mut place = base;
    let mut seen = false;
    for (key, field) in [("x", 0), ("cx", 0), ("y", 1), ("cy", 1), ("size", 2)] {
        if let Some(number) = values
            .get(&format!("{section}.{key}"))
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            match field {
                0 => place.cx = number,
                1 => place.cy = number,
                _ => place.size = number,
            }
            seen = true;
        }
    }
    if !seen {
        return None;
    }
    place.sanitize();
    Some(place)
}

fn word(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

/// A flat `section.key = value` map from a config file: `[layout]` and
/// `[button.attack]` headers, `#` comments, `key = value` lines. Values stay
/// text; callers parse what they expect and ignore the rest.
pub fn entries(text: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let name = if section.is_empty() {
            key
        } else {
            format!("{section}.{key}")
        };
        values.insert(name, value.trim().to_owned());
    }
    values
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (index, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..index],
            _ => {}
        }
    }
    line
}

/// The text `config/touch_controller-client.toml` starts with.
pub const DEFAULT_CONFIG: &str = "\
# TouchController for Horizon: where each on-screen control sits.
# Centre: a fraction of the surface, x across and y down; size: a fraction of
# its height. Delete a line to take the built-in value back.
#
# The same values are in Options > Mods as sliders (size, opacity, deadzone,
# left-handed) and a choice (joystick or D-pad); those apply on top of this file.

[layout]
# joystick or dpad
mode = \"joystick\"

[stick]
x = 0.170
y = 0.700
size = 0.230

[button.attack]
x = 0.855
y = 0.855
size = 0.093

[button.use]
x = 0.955
y = 0.690
size = 0.078

[button.jump]
x = 0.745
y = 0.735
size = 0.081

[button.sneak]
x = 0.090
y = 0.495
size = 0.064

[button.sprint]
x = 0.215
y = 0.560
size = 0.064

[button.inventory]
x = 0.950
y = 0.105
size = 0.058

[button.swap]
x = 0.845
y = 0.105
size = 0.058
";

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: f32 = 2.16;

    #[test]
    fn the_builtin_layout_keeps_the_centre_and_the_hotbar_clear() {
        let layout = Layout::builtin();
        for place in std::iter::once(layout.stick).chain(layout.buttons) {
            let [x, y, w, h] = place.rect(PHONE);
            assert!(
                x >= 0.0 && y >= 0.0 && x + w <= 1.0 && y + h <= 1.0,
                "{place:?} off screen"
            );
        }
        // The bar is 182x22 GUI pixels at the bottom centre; nothing covers it.
        for place in &layout.buttons {
            let [x, y, w, h] = place.rect(PHONE);
            let overlaps_bar = y + h > 0.93 && x < 0.62 && x + w > 0.38;
            assert!(!overlaps_bar, "{place:?} covers the hotbar");
        }
    }

    #[test]
    fn a_touch_centre_of_a_button_hits_it_and_the_stick_is_separate() {
        let layout = Layout::builtin();
        for button in BUTTONS {
            let place = layout.buttons[button.index()];
            assert_eq!(layout.button_at([place.cx, place.cy], PHONE), Some(button));
        }
        assert!(layout.on_stick([layout.stick.cx, layout.stick.cy], PHONE));
        assert!(!layout.on_stick([0.5, 0.1], PHONE));
    }

    #[test]
    fn the_stick_reads_forward_up_and_strafe_right() {
        let layout = Layout::builtin();
        let radius = layout.stick.size / 2.0;
        let up = [layout.stick.cx, layout.stick.cy - radius];
        let [forward, strafe] = layout.stick_vector(up, PHONE, 0.15);
        assert!(forward > 0.9 && strafe.abs() < 0.01, "{forward} {strafe}");
        let right = [layout.stick.cx + radius / PHONE, layout.stick.cy];
        let [forward, strafe] = layout.stick_vector(right, PHONE, 0.15);
        assert!(strafe > 0.9 && forward.abs() < 0.01, "{forward} {strafe}");
        let centre = [layout.stick.cx, layout.stick.cy];
        assert_eq!(layout.stick_vector(centre, PHONE, 0.15), [0.0, 0.0]);
    }

    #[test]
    fn a_config_file_moves_one_control_and_keeps_the_rest() {
        let layout =
            Layout::parse("[button.jump]\nx = 0.4\ny = 0.4\nsize = 0.1\n[unknown]\nnope = 1\n");
        assert_eq!(
            layout.buttons[Button::Jump.index()],
            Place::new(0.4, 0.4, 0.1)
        );
        assert_eq!(
            layout.buttons[Button::Attack.index()],
            Button::Attack.default_place()
        );
        assert_eq!(
            Layout::parse("[layout]\nmode = \"dpad\"\n").mode,
            Mode::Dpad
        );
        assert_eq!(Layout::parse("").stick, Layout::builtin().stick);
    }

    #[test]
    fn mirroring_swaps_the_sides_and_scale_resizes() {
        let mut layout = Layout::builtin();
        let attack = layout.buttons[Button::Attack.index()];
        layout.mirror();
        assert!((layout.buttons[Button::Attack.index()].cx - (1.0 - attack.cx)).abs() < 1e-6);
        let mut layout = Layout::builtin();
        let before = layout.stick.size;
        layout.scale(1.2);
        assert!((layout.stick.size - before * 1.2).abs() < 1e-6);
    }

    #[test]
    fn a_config_value_out_of_range_is_pulled_back_on_screen() {
        let layout = Layout::parse("[stick]\nx = 4.0\ny = -3.0\nsize = 9.0\n");
        assert!(layout.stick.cx <= 0.98 && layout.stick.cy >= 0.02);
        assert!(layout.stick.size <= 0.30);
    }

    #[test]
    fn the_default_config_file_describes_the_builtin_layout() {
        assert_eq!(Layout::parse(DEFAULT_CONFIG), Layout::builtin());
    }
}
