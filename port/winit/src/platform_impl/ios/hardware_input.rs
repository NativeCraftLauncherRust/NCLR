//! Hardware keyboard and mouse events from the GameController framework.
//!
//! UIKit gives a game view no reliable responder for a Bluetooth keyboard and
//! no relative mouse motion while the pointer is hidden. The iOS shell owns
//! GCKeyboard and GCMouse and forwards their HID events here; they enter the
//! same winit event stream as a desktop keyboard and mouse.
use objc2_foundation::MainThreadMarker;
use smol_str::SmolStr;

use super::app_state::{self, EventWrapper};
use super::DEVICE_ID;
use crate::dpi::PhysicalPosition;
use crate::event::{
    DeviceEvent, ElementState, Event, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, TouchPhase,
    WindowEvent,
};
use crate::keyboard::{
    Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey,
};
use crate::platform_impl::KeyEventExtra;

const MOD_SHIFT: u8 = 1;
const MOD_CONTROL: u8 = 2;
const MOD_ALT: u8 = 4;
const MOD_SUPER: u8 = 8;
const MOD_CAPS: u8 = 16;

enum LogicalKey {
    Character(char, char),
    Named(NamedKey),
}

fn modifiers_state(bits: u8) -> ModifiersState {
    let mut state = ModifiersState::empty();
    state.set(ModifiersState::SHIFT, bits & MOD_SHIFT != 0);
    state.set(ModifiersState::CONTROL, bits & MOD_CONTROL != 0);
    state.set(ModifiersState::ALT, bits & MOD_ALT != 0);
    state.set(ModifiersState::SUPER, bits & MOD_SUPER != 0);
    state
}

fn key_spec(usage: u16) -> Option<(KeyCode, LogicalKey, KeyLocation)> {
    const LETTERS: [(KeyCode, char, char); 26] = [
        (KeyCode::KeyA, 'a', 'A'),
        (KeyCode::KeyB, 'b', 'B'),
        (KeyCode::KeyC, 'c', 'C'),
        (KeyCode::KeyD, 'd', 'D'),
        (KeyCode::KeyE, 'e', 'E'),
        (KeyCode::KeyF, 'f', 'F'),
        (KeyCode::KeyG, 'g', 'G'),
        (KeyCode::KeyH, 'h', 'H'),
        (KeyCode::KeyI, 'i', 'I'),
        (KeyCode::KeyJ, 'j', 'J'),
        (KeyCode::KeyK, 'k', 'K'),
        (KeyCode::KeyL, 'l', 'L'),
        (KeyCode::KeyM, 'm', 'M'),
        (KeyCode::KeyN, 'n', 'N'),
        (KeyCode::KeyO, 'o', 'O'),
        (KeyCode::KeyP, 'p', 'P'),
        (KeyCode::KeyQ, 'q', 'Q'),
        (KeyCode::KeyR, 'r', 'R'),
        (KeyCode::KeyS, 's', 'S'),
        (KeyCode::KeyT, 't', 'T'),
        (KeyCode::KeyU, 'u', 'U'),
        (KeyCode::KeyV, 'v', 'V'),
        (KeyCode::KeyW, 'w', 'W'),
        (KeyCode::KeyX, 'x', 'X'),
        (KeyCode::KeyY, 'y', 'Y'),
        (KeyCode::KeyZ, 'z', 'Z'),
    ];
    const DIGITS: [(KeyCode, char, char); 10] = [
        (KeyCode::Digit1, '1', '!'),
        (KeyCode::Digit2, '2', '@'),
        (KeyCode::Digit3, '3', '#'),
        (KeyCode::Digit4, '4', '$'),
        (KeyCode::Digit5, '5', '%'),
        (KeyCode::Digit6, '6', '^'),
        (KeyCode::Digit7, '7', '&'),
        (KeyCode::Digit8, '8', '*'),
        (KeyCode::Digit9, '9', '('),
        (KeyCode::Digit0, '0', ')'),
    ];
    if (0x04..=0x1D).contains(&usage) {
        let (code, lower, upper) = LETTERS[usize::from(usage - 0x04)];
        return Some((code, LogicalKey::Character(lower, upper), KeyLocation::Standard));
    }
    if (0x1E..=0x27).contains(&usage) {
        let (code, lower, upper) = DIGITS[usize::from(usage - 0x1E)];
        return Some((code, LogicalKey::Character(lower, upper), KeyLocation::Standard));
    }
    let character = |code, lower, upper| {
        Some((code, LogicalKey::Character(lower, upper), KeyLocation::Standard))
    };
    let named = |code, name, location| Some((code, LogicalKey::Named(name), location));
    match usage {
        0x28 => named(KeyCode::Enter, NamedKey::Enter, KeyLocation::Standard),
        0x29 => named(KeyCode::Escape, NamedKey::Escape, KeyLocation::Standard),
        0x2A => named(KeyCode::Backspace, NamedKey::Backspace, KeyLocation::Standard),
        0x2B => named(KeyCode::Tab, NamedKey::Tab, KeyLocation::Standard),
        0x2C => named(KeyCode::Space, NamedKey::Space, KeyLocation::Standard),
        0x2D => character(KeyCode::Minus, '-', '_'),
        0x2E => character(KeyCode::Equal, '=', '+'),
        0x2F => character(KeyCode::BracketLeft, '[', '{'),
        0x30 => character(KeyCode::BracketRight, ']', '}'),
        0x31 => character(KeyCode::Backslash, '\\', '|'),
        0x33 => character(KeyCode::Semicolon, ';', ':'),
        0x34 => character(KeyCode::Quote, '\'', '"'),
        0x35 => character(KeyCode::Backquote, '\u{60}', '~'),
        0x36 => character(KeyCode::Comma, ',', '<'),
        0x37 => character(KeyCode::Period, '.', '>'),
        0x38 => character(KeyCode::Slash, '/', '?'),
        0x39 => named(KeyCode::CapsLock, NamedKey::CapsLock, KeyLocation::Standard),
        0x3A..=0x45 => {
            const F_KEYS: [(KeyCode, NamedKey); 12] = [
                (KeyCode::F1, NamedKey::F1),
                (KeyCode::F2, NamedKey::F2),
                (KeyCode::F3, NamedKey::F3),
                (KeyCode::F4, NamedKey::F4),
                (KeyCode::F5, NamedKey::F5),
                (KeyCode::F6, NamedKey::F6),
                (KeyCode::F7, NamedKey::F7),
                (KeyCode::F8, NamedKey::F8),
                (KeyCode::F9, NamedKey::F9),
                (KeyCode::F10, NamedKey::F10),
                (KeyCode::F11, NamedKey::F11),
                (KeyCode::F12, NamedKey::F12),
            ];
            let (code, name) = F_KEYS[usize::from(usage - 0x3A)];
            named(code, name, KeyLocation::Standard)
        }
        0x49 => named(KeyCode::Insert, NamedKey::Insert, KeyLocation::Standard),
        0x4A => named(KeyCode::Home, NamedKey::Home, KeyLocation::Standard),
        0x4B => named(KeyCode::PageUp, NamedKey::PageUp, KeyLocation::Standard),
        0x4C => named(KeyCode::Delete, NamedKey::Delete, KeyLocation::Standard),
        0x4D => named(KeyCode::End, NamedKey::End, KeyLocation::Standard),
        0x4E => named(KeyCode::PageDown, NamedKey::PageDown, KeyLocation::Standard),
        0x4F => named(KeyCode::ArrowRight, NamedKey::ArrowRight, KeyLocation::Standard),
        0x50 => named(KeyCode::ArrowLeft, NamedKey::ArrowLeft, KeyLocation::Standard),
        0x51 => named(KeyCode::ArrowDown, NamedKey::ArrowDown, KeyLocation::Standard),
        0x52 => named(KeyCode::ArrowUp, NamedKey::ArrowUp, KeyLocation::Standard),
        0xE0 => named(KeyCode::ControlLeft, NamedKey::Control, KeyLocation::Left),
        0xE1 => named(KeyCode::ShiftLeft, NamedKey::Shift, KeyLocation::Left),
        0xE2 => named(KeyCode::AltLeft, NamedKey::Alt, KeyLocation::Left),
        0xE3 => named(KeyCode::SuperLeft, NamedKey::Super, KeyLocation::Left),
        0xE4 => named(KeyCode::ControlRight, NamedKey::Control, KeyLocation::Right),
        0xE5 => named(KeyCode::ShiftRight, NamedKey::Shift, KeyLocation::Right),
        0xE6 => named(KeyCode::AltRight, NamedKey::Alt, KeyLocation::Right),
        0xE7 => named(KeyCode::SuperRight, NamedKey::Super, KeyLocation::Right),
        _ => None,
    }
}

pub(crate) fn send_key(usage: u16, pressed: bool, repeat: bool, modifiers: u8) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window_id) = app_state::key_window_id() else {
        return;
    };
    let state = modifiers_state(modifiers);
    let mut events = vec![EventWrapper::StaticEvent(Event::WindowEvent {
        window_id,
        event: WindowEvent::ModifiersChanged(Modifiers::from(state)),
    })];
    if let Some((code, logical, location)) = key_spec(usage) {
        let shift = modifiers & MOD_SHIFT != 0;
        let caps = modifiers & MOD_CAPS != 0;
        let (logical_key, text) = match logical {
            LogicalKey::Character(lower, upper) => {
                let ch = if shift ^ caps { upper } else { lower };
                let text = (pressed && modifiers & (MOD_CONTROL | MOD_ALT | MOD_SUPER) == 0)
                    .then(|| SmolStr::new(ch.to_string()));
                (Key::Character(SmolStr::new(ch.to_string())), text)
            }
            LogicalKey::Named(name) => (Key::Named(name), None),
        };
        events.push(EventWrapper::StaticEvent(Event::WindowEvent {
            window_id,
            event: WindowEvent::KeyboardInput {
                device_id: DEVICE_ID,
                is_synthetic: false,
                event: KeyEvent {
                    physical_key: PhysicalKey::Code(code),
                    logical_key,
                    text,
                    location,
                    state: if pressed {
                        ElementState::Pressed
                    } else {
                        ElementState::Released
                    },
                    repeat,
                    platform_specific: KeyEventExtra {},
                },
            },
        }));
    }
    app_state::handle_nonuser_events(mtm, events);
}

pub(crate) fn send_mouse_motion(dx: f64, dy: f64, x: f64, y: f64) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window_id) = app_state::key_window_id() else {
        return;
    };
    app_state::handle_nonuser_events(
        mtm,
        [
            EventWrapper::StaticEvent(Event::DeviceEvent {
                device_id: DEVICE_ID,
                event: DeviceEvent::MouseMotion { delta: (dx, dy) },
            }),
            EventWrapper::StaticEvent(Event::WindowEvent {
                window_id,
                event: WindowEvent::CursorMoved {
                    device_id: DEVICE_ID,
                    position: PhysicalPosition::new(x, y),
                },
            }),
        ],
    );
}

pub(crate) fn send_mouse_button(button: u8, pressed: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window_id) = app_state::key_window_id() else {
        return;
    };
    let button = match button {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        _ => return,
    };
    app_state::handle_nonuser_event(
        mtm,
        EventWrapper::StaticEvent(Event::WindowEvent {
            window_id,
            event: WindowEvent::MouseInput {
                device_id: DEVICE_ID,
                state: if pressed {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
                button,
            },
        }),
    );
}

pub(crate) fn send_mouse_scroll(dx: f64, dy: f64) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window_id) = app_state::key_window_id() else {
        return;
    };
    app_state::handle_nonuser_event(
        mtm,
        EventWrapper::StaticEvent(Event::WindowEvent {
            window_id,
            event: WindowEvent::MouseWheel {
                device_id: DEVICE_ID,
                delta: MouseScrollDelta::LineDelta(dx as f32, dy as f32),
                phase: TouchPhase::Moved,
            },
        }),
    );
}
