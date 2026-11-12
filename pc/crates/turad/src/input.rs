//! Virtual mouse + keyboard via /dev/uinput. Works on Wayland and X11.
//!
//! Needs write access to /dev/uinput, see `pc/packaging/60-tunnelrat-uinput.rules`.

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};
use tura_proto::{MouseButton, PressState};

pub struct Input {
    dev: VirtualDevice,
    /// Sub-pixel remainders so slow finger movement isn't lost to rounding.
    rem_x: f32,
    rem_y: f32,
    rem_sx: f32,
    rem_sy: f32,
}

impl Input {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        keys.insert(KeyCode::BTN_LEFT);
        keys.insert(KeyCode::BTN_RIGHT);
        keys.insert(KeyCode::BTN_MIDDLE);
        // Every regular keyboard key (KEY_ESC=1 .. KEY_MICMUTE=248).
        for code in 1..=248u16 {
            keys.insert(KeyCode::new(code));
        }

        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        axes.insert(RelativeAxisCode::REL_X);
        axes.insert(RelativeAxisCode::REL_Y);
        axes.insert(RelativeAxisCode::REL_WHEEL);
        axes.insert(RelativeAxisCode::REL_HWHEEL);

        let dev = VirtualDevice::builder()
            .context("cannot open /dev/uinput — is the udev rule installed and are you in the 'uinput' group? (pc/packaging/60-tunnelrat-uinput.rules)")?
            .name("Tunnelrat virtual input")
            .with_keys(&keys)?
            .with_relative_axes(&axes)?
            .build()?;

        Ok(Self { dev, rem_x: 0.0, rem_y: 0.0, rem_sx: 0.0, rem_sy: 0.0 })
    }

    pub fn pointer_move(&mut self, dx: f32, dy: f32) -> Result<()> {
        let x = take_whole(&mut self.rem_x, dx);
        let y = take_whole(&mut self.rem_y, dy);
        self.rel(&[(RelativeAxisCode::REL_X, x), (RelativeAxisCode::REL_Y, y)])
    }

    /// `dy` > 0 scrolls down, in wheel "clicks".
    pub fn scroll(&mut self, dx: f32, dy: f32) -> Result<()> {
        let x = take_whole(&mut self.rem_sx, dx);
        let y = take_whole(&mut self.rem_sy, dy);
        // evdev wheel is positive = up, so invert.
        self.rel(&[(RelativeAxisCode::REL_HWHEEL, x), (RelativeAxisCode::REL_WHEEL, -y)])
    }

    pub fn button(&mut self, button: MouseButton, state: PressState) -> Result<()> {
        let key = match button {
            MouseButton::Left => KeyCode::BTN_LEFT,
            MouseButton::Right => KeyCode::BTN_RIGHT,
            MouseButton::Middle => KeyCode::BTN_MIDDLE,
        };
        self.press(key, state)
    }

    pub fn key(&mut self, code: u16, state: PressState) -> Result<()> {
        self.press(KeyCode::new(code), state)
    }

    /// Types layout key presses (from `keymap`) while holding `held` keys
    /// (shortcut modifiers like Ctrl). One SYN report per event so the
    /// compositor sees modifier → key → release in order.
    pub fn type_presses(&mut self, presses: &[crate::keymap::Press], held: &[u16]) -> Result<()> {
        use crate::keymap::{KEY_LEFTSHIFT, KEY_RIGHTALT};
        for &h in held {
            self.key_event(h, 1)?;
        }
        let res = (|| {
            for p in presses {
                let mut mods = vec![];
                if p.shift && !held.contains(&KEY_LEFTSHIFT) {
                    mods.push(KEY_LEFTSHIFT);
                }
                if p.altgr {
                    mods.push(KEY_RIGHTALT);
                }
                for &m in &mods {
                    self.key_event(m, 1)?;
                }
                self.key_event(p.key, 1)?;
                self.key_event(p.key, 0)?;
                for &m in mods.iter().rev() {
                    self.key_event(m, 0)?;
                }
            }
            Ok(())
        })();
        // Always release held modifiers, even after an error.
        for &h in held.iter().rev() {
            let _ = self.key_event(h, 0);
        }
        res
    }

    fn key_event(&mut self, code: u16, value: i32) -> Result<()> {
        self.dev.emit(&[InputEvent::new(EventType::KEY.0, code, value)])?;
        Ok(())
    }

    fn press(&mut self, key: KeyCode, state: PressState) -> Result<()> {
        let ev = |v| InputEvent::new(EventType::KEY.0, key.0, v);
        match state {
            PressState::Down => self.dev.emit(&[ev(1)])?,
            PressState::Up => self.dev.emit(&[ev(0)])?,
            PressState::Click => {
                self.dev.emit(&[ev(1)])?;
                self.dev.emit(&[ev(0)])?;
            }
        }
        Ok(())
    }

    fn rel(&mut self, moves: &[(RelativeAxisCode, i32)]) -> Result<()> {
        let events: Vec<_> = moves
            .iter()
            .filter(|(_, v)| *v != 0)
            .map(|(axis, v)| InputEvent::new(EventType::RELATIVE.0, axis.0, *v))
            .collect();
        if !events.is_empty() {
            self.dev.emit(&events)?;
        }
        Ok(())
    }
}

/// Adds `delta` to `rem` and returns the whole part, keeping the fraction.
fn take_whole(rem: &mut f32, delta: f32) -> i32 {
    *rem += delta;
    let whole = rem.trunc();
    *rem -= whole;
    whole as i32
}

#[cfg(test)]
mod tests {
    use super::take_whole;

    #[test]
    fn subpixel_accumulates() {
        let mut r = 0.0;
        assert_eq!(take_whole(&mut r, 0.4), 0);
        assert_eq!(take_whole(&mut r, 0.4), 0);
        assert_eq!(take_whole(&mut r, 0.4), 1);
        assert_eq!(take_whole(&mut r, -1.5), -1);
    }
}
