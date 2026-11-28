//! Merge the UIKit touch overlay and physical devices before emitting desktop input.

#[derive(Default)]
pub(super) struct InputSources {
    keys: [std::collections::BTreeSet<u16>; 2],
    buttons: [std::collections::BTreeSet<u8>; 2],
    hardware_modifiers: u8,
}

impl InputSources {
    pub(super) fn key(&mut self, source: usize, usage: u16, down: bool, repeat: bool) -> bool {
        let held = self.keys.iter().any(|keys| keys.contains(&usage));
        let changed = if down {
            self.keys[source].insert(usage)
        } else {
            self.keys[source].remove(&usage)
        };
        let now = self.keys.iter().any(|keys| keys.contains(&usage));
        (changed && held != now) || (down && repeat && self.keys[source].contains(&usage))
    }

    pub(super) fn button(&mut self, source: usize, button: u8, down: bool) -> bool {
        let held = self.buttons.iter().any(|buttons| buttons.contains(&button));
        if down {
            self.buttons[source].insert(button);
        } else {
            self.buttons[source].remove(&button);
        }
        held != self.buttons.iter().any(|buttons| buttons.contains(&button))
    }

    pub(super) fn modifiers(&mut self, source: usize, modifiers: u8) -> u8 {
        if source == 0 {
            self.hardware_modifiers = modifiers;
        }
        let touch = &self.keys[1];
        let mut combined = self.hardware_modifiers;
        for (usages, bit) in [
            ([0xE1, 0xE5], 1),
            ([0xE0, 0xE4], 2),
            ([0xE2, 0xE6], 4),
            ([0xE3, 0xE7], 8),
        ] {
            if usages.iter().any(|usage| touch.contains(usage)) {
                combined |= bit;
            }
        }
        combined
    }

    pub(super) fn touch_held(&self) -> (Vec<u16>, Vec<u8>) {
        (
            self.keys[1].iter().copied().collect(),
            self.buttons[1].iter().copied().collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_touch_preserves_physical_mining() {
        let mut input = InputSources::default();
        assert!(input.button(0, 0, true));
        assert!(!input.button(1, 0, true));
        assert!(!input.button(1, 0, false));
        assert!(input.button(0, 0, false));
    }

    #[test]
    fn disconnecting_mouse_preserves_touch_mining() {
        let mut input = InputSources::default();
        assert!(input.button(1, 0, true));
        assert!(!input.button(0, 0, true));
        assert!(!input.button(0, 0, false));
        assert!(input.button(1, 0, false));
    }

    #[test]
    fn touch_hold_never_generates_repeated_presses() {
        let mut input = InputSources::default();
        for _ in 0..100 {
            assert!(input.button(1, 0, true));
            for _ in 0..120 {
                assert!(!input.button(1, 0, true));
            }
            assert!(input.button(1, 0, false));
            assert!(!input.button(1, 0, false));
        }
    }

    #[test]
    fn keyboard_and_touch_share_a_key_without_early_release() {
        let mut input = InputSources::default();
        assert!(input.key(1, 0x1A, true, false));
        assert!(!input.key(0, 0x1A, true, false));
        assert!(input.key(0, 0x1A, true, true));
        assert!(!input.key(0, 0x1A, false, false));
        assert!(input.key(1, 0x1A, false, false));
    }

    #[test]
    fn touch_modifiers_survive_physical_keyboard_reports() {
        let mut input = InputSources::default();
        input.key(1, 0xE1, true, false);
        assert_eq!(input.modifiers(1, 0), 1);
        assert_eq!(input.modifiers(0, 2), 3);
        input.key(1, 0xE1, false, false);
        assert_eq!(input.modifiers(1, 0), 2);
        assert_eq!(input.modifiers(0, 0), 0);
    }

    #[test]
    fn cancellation_releases_only_touch_owned_keys_and_buttons() {
        let mut input = InputSources::default();
        input.key(0, 0x1A, true, false);
        input.key(1, 0x1A, true, false);
        input.key(1, 0xE1, true, false);
        input.button(0, 0, true);
        input.button(1, 0, true);
        input.button(1, 1, true);
        let (keys, buttons) = input.touch_held();
        assert_eq!(keys, [0x1A, 0xE1]);
        assert_eq!(buttons, [0, 1]);
        for key in keys {
            assert_eq!(input.key(1, key, false, false), key != 0x1A);
        }
        for button in buttons {
            assert_eq!(input.button(1, button, false), button != 0);
        }
        assert_eq!(input.touch_held(), (Vec::new(), Vec::new()));
        assert!(input.key(0, 0x1A, false, false));
        assert!(input.button(0, 0, false));
    }
}
