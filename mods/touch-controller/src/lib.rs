//! TouchController for Horizon and NCLR: Bedrock-style touch controls.
//!
//! The mod draws its own stick (or D-pad) and buttons on the HUD and claims the
//! fingers that land on them, so the game's built-in touch layout never answers
//! the same finger. Everything else — the look drag, the hotbar taps, the pause
//! and settings buttons the player keeps visible — stays the game's own, which
//! is why a player hides only the duplicated controls in NCLR's touch editor.
//!
//! A press holds the vanilla mapping its button names (`key.attack`,
//! `key.jump`...), so the game acts through exactly the checks and packets of a
//! normal client: no server sees a mod. Movement is the analogue axes of
//! [`Hooks::movement_input`], which the game turns into the player's movement
//! input as it does for a controller.
//!
//! Controls, the config file and its defaults live in [`layout`]; the drawing
//! is [`ui`].
use mod_api::{
    ClientHost, ClientTickPreEvent, Config, ConfigKind, ConfigLoadingEvent, ConfigReloadingEvent,
    Flow, GuiRenderPostEvent, Hooks, HostSide, MovementInputEvent, RegisterConfigsEvent, Registrar,
    RegistrationContext, SettingSpec, TouchInputEvent,
};
use std::sync::{Mutex, MutexGuard};

mod layout;
mod ui;

use layout::{BUTTONS, Button, DEFAULT_CONFIG, Layout, Mode};

/// This mod's id: its settings, its config file and its package name.
const ID: &str = "touch_controller";

/// The settings the player changes in Options > Mods, read once per hook call.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    mode: Mode,
    scale: f32,
    opacity: f32,
    deadzone: f32,
    mirrored: bool,
    sneak_toggle: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Joystick,
            scale: 1.0,
            opacity: 0.7,
            deadzone: 0.16,
            mirrored: false,
            sneak_toggle: true,
        }
    }
}

/// Which control a finger owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Stick,
    Button(usize),
}

/// One finger the mod has claimed, for the length of its gesture.
#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    role: Role,
}

/// What the mod remembers between hooks.
#[derive(Default)]
struct State {
    fingers: Vec<Finger>,
    /// Held buttons, indexed like [`BUTTONS`].
    held: [bool; BUTTONS.len()],
    /// The stick's movement: forward, strafe, `-1..1`.
    stick: [f32; 2],
    stick_finger: Option<u64>,
    /// With the toggle option on, the sneak button latches instead of holding.
    sneak_latched: bool,
    /// A finger has landed since the game started: this is a touch screen.
    touch_seen: bool,
}

struct Inner {
    settings: Settings,
    /// The layout the config file describes.
    base: Layout,
    /// `base` with this frame's settings applied: what is drawn and hit-tested.
    shown: Layout,
    state: State,
}

impl Inner {
    fn relayout(&mut self) {
        let mut layout = self.base.clone();
        // The config file picks the default; the setting only ever adds the
        // D-pad, so a file that says D-pad is not undone by the slider's default.
        if self.settings.mode == Mode::Dpad {
            layout.mode = Mode::Dpad;
        }
        if self.settings.mirrored {
            layout.mirror();
        }
        layout.scale(self.settings.scale);
        self.shown = layout;
    }
}

/// The mod: the game's one `impl Hooks`.
pub struct TouchController {
    inner: Mutex<Inner>,
}

impl Default for TouchController {
    fn default() -> Self {
        Self::new()
    }
}

impl TouchController {
    pub fn new() -> Self {
        let base = Layout::builtin();
        Self {
            inner: Mutex::new(Inner {
                settings: Settings::default(),
                base: base.clone(),
                shown: base,
                state: State::default(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reads the player's settings; a change re-lays the controls out.
    fn refresh(&self, host: &dyn ClientHost) {
        let settings = Settings {
            mode: match host.setting_choice("layout") {
                Some(1) => Mode::Dpad,
                _ => Mode::Joystick,
            },
            scale: host.setting_f64("scale").unwrap_or(1.0) as f32,
            opacity: host.setting_f64("opacity").unwrap_or(0.7) as f32,
            deadzone: host.setting_f64("deadzone").unwrap_or(0.16) as f32,
            mirrored: host.setting_bool("left_handed").unwrap_or(false),
            sneak_toggle: host.setting_bool("sneak_toggle").unwrap_or(true),
        };
        let mut inner = self.lock();
        if inner.settings != settings {
            inner.settings = settings;
            inner.relayout();
        }
    }

    /// Keeps the config file's text: the layout it describes is the base every
    /// setting applies on top of.
    fn set_config(&self, text: &str) {
        let mut inner = self.lock();
        inner.base = Layout::parse(text);
        inner.relayout();
    }

    /// Holds or clicks a button's mapping as its finger lands.
    fn press(host: &mut dyn ClientHost, inner: &mut Inner, button: Button) {
        let index = button.index();
        if inner.state.held[index] {
            return;
        }
        inner.state.held[index] = true;
        if button == Button::Sneak && inner.settings.sneak_toggle {
            inner.state.sneak_latched = !inner.state.sneak_latched;
            let _ = host.set_mapping_down(button.mapping(), inner.state.sneak_latched);
            return;
        }
        if button.click_only() {
            let _ = host.click_mapping(button.mapping());
            return;
        }
        // A click starts attacking or using at once; the held mapping carries it
        // on for as long as the finger stays down, as a held key does.
        let _ = host.click_mapping(button.mapping());
        let _ = host.set_mapping_down(button.mapping(), true);
    }

    /// Lets a button's mapping go when its last finger lifts.
    fn release(host: &mut dyn ClientHost, inner: &mut Inner, button: Button) {
        let index = button.index();
        inner.state.held[index] = false;
        if button == Button::Sneak && inner.settings.sneak_toggle {
            return;
        }
        if button.click_only() {
            return;
        }
        let _ = host.set_mapping_down(button.mapping(), false);
    }

    /// Drops every finger and held mapping, for a world that closes under them.
    fn release_all(&self, host: &mut dyn ClientHost) {
        let mut inner = self.lock();
        if inner.state.fingers.is_empty()
            && inner.state.stick == [0.0, 0.0]
            && !inner.state.held.iter().any(|held| *held)
            && !inner.state.sneak_latched
        {
            return;
        }
        inner.state.fingers.clear();
        inner.state.stick = [0.0, 0.0];
        inner.state.stick_finger = None;
        inner.state.sneak_latched = false;
        for button in BUTTONS {
            if inner.state.held[button.index()] && !button.click_only() {
                let _ = host.set_mapping_down(button.mapping(), false);
            }
            inner.state.held[button.index()] = false;
        }
    }
}

#[mod_api::horizon_mod]
impl Hooks for TouchController {
    fn register(&self, registrar: &mut dyn Registrar, _cx: &RegistrationContext) {
        let settings = [
            SettingSpec::choice(
                "layout",
                "touch_controller.setting.layout",
                [
                    "touch_controller.layout.joystick",
                    "touch_controller.layout.dpad",
                ],
                0,
            )
            .tooltip("touch_controller.setting.layout.tooltip"),
            SettingSpec::range(
                "scale",
                "touch_controller.setting.scale",
                0.6,
                1.6,
                0.05,
                1.0,
            )
            .tooltip("touch_controller.setting.scale.tooltip"),
            SettingSpec::range(
                "opacity",
                "touch_controller.setting.opacity",
                0.2,
                1.0,
                0.05,
                0.7,
            )
            .tooltip("touch_controller.setting.opacity.tooltip"),
            SettingSpec::range(
                "deadzone",
                "touch_controller.setting.deadzone",
                0.0,
                0.45,
                0.01,
                0.16,
            )
            .tooltip("touch_controller.setting.deadzone.tooltip"),
            SettingSpec::toggle("left_handed", "touch_controller.setting.left_handed", false)
                .tooltip("touch_controller.setting.left_handed.tooltip"),
            SettingSpec::toggle(
                "sneak_toggle",
                "touch_controller.setting.sneak_toggle",
                true,
            )
            .tooltip("touch_controller.setting.sneak_toggle.tooltip"),
        ];
        for setting in settings {
            let _ = registrar.register_setting(setting);
        }
    }

    fn register_configs(&self, _host: HostSide, event: &mut RegisterConfigsEvent<'_>) {
        event
            .configs
            .push(Config::new(ID, ConfigKind::Client, DEFAULT_CONFIG));
    }

    fn config_loading(&self, _host: HostSide, event: &mut ConfigLoadingEvent<'_>) {
        if event.config.mod_id == ID {
            self.set_config(&event.config.text);
        }
    }

    fn config_reloading(&self, _host: HostSide, event: &mut ConfigReloadingEvent<'_>) {
        if event.config.mod_id == ID {
            self.set_config(&event.config.text);
        }
    }

    fn client_tick_pre(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPreEvent) {
        if !host.on(ID) || !host.playing() {
            self.release_all(host);
        }
    }

    fn touch_input(&self, host: &mut dyn ClientHost, event: &mut TouchInputEvent) -> Flow {
        if !host.on(ID) || !host.playing() {
            return Flow::Continue;
        }
        self.refresh(host);
        let aspect = event.surface[0].max(1) as f32 / event.surface[1].max(1) as f32;
        let point = event.point;
        let mut inner = self.lock();
        inner.state.touch_seen = true;
        let layout = inner.shown.clone();
        let deadzone = inner.settings.deadzone;
        match event.phase {
            // A finger landed: a button takes it, else the pad, else it is the
            // game's (the look drag, the hotbar, a menu button).
            0 => {
                if let Some(button) = layout.button_at(point, aspect) {
                    inner.state.fingers.push(Finger {
                        id: event.id,
                        role: Role::Button(button.index()),
                    });
                    event.cancelled = true;
                    Self::press(host, &mut inner, button);
                } else if layout.on_stick(point, aspect) {
                    inner.state.fingers.push(Finger {
                        id: event.id,
                        role: Role::Stick,
                    });
                    inner.state.stick_finger = Some(event.id);
                    inner.state.stick = match layout.mode {
                        Mode::Joystick => layout.stick_vector(point, aspect, deadzone),
                        Mode::Dpad => layout.dpad(point, aspect, deadzone),
                    };
                    event.cancelled = true;
                }
            }
            // It moved: a stick finger keeps following the pad.
            1 => {
                if inner
                    .state
                    .fingers
                    .iter()
                    .any(|finger| finger.id == event.id && finger.role == Role::Stick)
                {
                    inner.state.stick = match layout.mode {
                        Mode::Joystick => layout.stick_vector(point, aspect, deadzone),
                        Mode::Dpad => layout.dpad(point, aspect, deadzone),
                    };
                    event.cancelled = true;
                } else if inner
                    .state
                    .fingers
                    .iter()
                    .any(|finger| finger.id == event.id)
                {
                    // A button's finger sliding off keeps its button: letting go
                    // by sliding is the one gesture players make by accident.
                    event.cancelled = true;
                }
            }
            // It lifted or the system cancelled it.
            _ => {
                let Some(index) = inner
                    .state
                    .fingers
                    .iter()
                    .position(|finger| finger.id == event.id)
                else {
                    return Flow::Continue;
                };
                let finger = inner.state.fingers.swap_remove(index);
                event.cancelled = true;
                match finger.role {
                    Role::Stick => {
                        inner.state.stick = [0.0, 0.0];
                        if inner.state.stick_finger == Some(event.id) {
                            inner.state.stick_finger = None;
                        }
                    }
                    Role::Button(index) => {
                        let still_held = inner
                            .state
                            .fingers
                            .iter()
                            .any(|finger| finger.role == Role::Button(index));
                        if !still_held {
                            Self::release(host, &mut inner, BUTTONS[index]);
                        }
                    }
                }
            }
        }
        Flow::Continue
    }

    fn movement_input(&self, host: &dyn ClientHost, event: &mut MovementInputEvent) {
        if !host.on(ID) || !host.playing() {
            return;
        }
        self.refresh(host);
        let inner = self.lock();
        if inner.state.held[Button::Sprint.index()] {
            event.sprint = true;
        }
        if inner.state.stick == [0.0, 0.0] {
            return;
        }
        let [forward, strafe] = inner.state.stick;
        match inner.shown.mode {
            Mode::Joystick => {
                event.forward = forward;
                event.strafe = strafe;
            }
            Mode::Dpad => {
                event.forward_key = forward > 0.5;
                event.backward_key = forward < -0.5;
                event.right_key = strafe > 0.5;
                event.left_key = strafe < -0.5;
            }
        }
    }

    fn gui_render_post(&self, host: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
        if !host.on(ID) || !host.playing() {
            return;
        }
        self.refresh(host);
        let inner = self.lock();
        if !inner.state.touch_seen {
            return;
        }
        let label = |button: Button| {
            host.translate(button.label_key())
                .unwrap_or_else(|| fallback(button).to_owned())
        };
        ui::draw(
            event.batch,
            &inner.shown,
            &ui::View {
                stick: inner.state.stick,
                held: &inner.state.held,
                stick_active: inner.state.stick_finger.is_some(),
                opacity: inner.settings.opacity,
                label: &label,
            },
        );
    }
}

/// What a button says before the language file is read.
fn fallback(button: Button) -> &'static str {
    match button {
        Button::Attack => "A",
        Button::Use => "U",
        Button::Jump => "J",
        Button::Sneak => "S",
        Button::Sprint => "R",
        Button::Inventory => "I",
        Button::Swap => "W",
    }
}
