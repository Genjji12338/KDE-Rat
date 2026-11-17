//! Gamepad mode: the phone becomes a virtual Xbox 360 controller (uinput),
//! so games and Steam see a normal gamepad. One per phone, created on its
//! first `Gamepad` message and unplugged on `GamepadEnd` or disconnect.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, UinputAbsSetup};
use tura_proto::pad;

/// (protocol bit, evdev button) — like the kernel's xpad driver.
const BUTTONS: [(u32, KeyCode); 11] = [
    (pad::A, KeyCode::BTN_SOUTH),
    (pad::B, KeyCode::BTN_EAST),
    (pad::X, KeyCode::BTN_NORTH),
    (pad::Y, KeyCode::BTN_WEST),
    (pad::LB, KeyCode::BTN_TL),
    (pad::RB, KeyCode::BTN_TR),
    (pad::VIEW, KeyCode::BTN_SELECT),
    (pad::MENU, KeyCode::BTN_START),
    (pad::HOME, KeyCode::BTN_MODE),
    (pad::L3, KeyCode::BTN_THUMBL),
    (pad::R3, KeyCode::BTN_THUMBR),
];

#[derive(Default, Clone, Copy, PartialEq)]
pub struct State {
    pub buttons: u32,
    /// Sticks −1…1 (y down), triggers 0…1.
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub lt: f32,
    pub rt: f32,
}

pub struct Gamepad {
    dev: VirtualDevice,
    last: [i32; 8],
    buttons: u32,
}

const STICK: i32 = 32767;
const TRIGGER: i32 = 255;

impl Gamepad {
    pub fn new(phone: &str) -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for (_, k) in BUTTONS {
            keys.insert(k);
        }
        let stick = AbsInfo::new(0, -STICK - 1, STICK, 16, 128, 0);
        let trigger = AbsInfo::new(0, 0, TRIGGER, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let name = format!("Tunnelrat gamepad ({phone})");
        let mut b = VirtualDevice::builder()
            .context("cannot open /dev/uinput (see pc/packaging/60-tunnelrat-uinput.rules)")?
            .name(&name)
            // Xbox 360 controller ids: games and Steam map it without setup.
            .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0110))
            .with_keys(&keys)?;
        for (axis, info) in [
            (Abs::ABS_X, stick),
            (Abs::ABS_Y, stick),
            (Abs::ABS_RX, stick),
            (Abs::ABS_RY, stick),
            (Abs::ABS_Z, trigger),
            (Abs::ABS_RZ, trigger),
            (Abs::ABS_HAT0X, hat),
            (Abs::ABS_HAT0Y, hat),
        ] {
            b = b.with_absolute_axis(&UinputAbsSetup::new(axis, info))?;
        }
        Ok(Self { dev: b.build()?, last: [0; 8], buttons: 0 })
    }

    /// Emits what changed since the last state, in one report.
    pub fn update(&mut self, s: State) -> Result<()> {
        let stick = |v: f32| (v.clamp(-1.0, 1.0) * STICK as f32).round() as i32;
        let trig = |v: f32| (v.clamp(0.0, 1.0) * TRIGGER as f32).round() as i32;
        let on = |bit: u32| s.buttons & bit != 0;
        let hat = |neg: u32, pos: u32| i32::from(on(pos)) - i32::from(on(neg));
        let axes = [
            (Abs::ABS_X, stick(s.lx)),
            (Abs::ABS_Y, stick(s.ly)),
            (Abs::ABS_RX, stick(s.rx)),
            (Abs::ABS_RY, stick(s.ry)),
            (Abs::ABS_Z, trig(s.lt)),
            (Abs::ABS_RZ, trig(s.rt)),
            (Abs::ABS_HAT0X, hat(pad::LEFT, pad::RIGHT)),
            (Abs::ABS_HAT0Y, hat(pad::UP, pad::DOWN)),
        ];
        let mut events = Vec::new();
        for (i, (axis, v)) in axes.into_iter().enumerate() {
            if self.last[i] != v {
                self.last[i] = v;
                events.push(InputEvent::new(EventType::ABSOLUTE.0, axis.0, v));
            }
        }
        let changed = self.buttons ^ s.buttons;
        for (bit, key) in BUTTONS {
            if changed & bit != 0 {
                events.push(InputEvent::new(EventType::KEY.0, key.0, i32::from(on(bit))));
            }
        }
        self.buttons = s.buttons;
        if !events.is_empty() {
            self.dev.emit(&events)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs /dev/uinput access: `cargo test -- --ignored gamepad`.
    #[test]
    #[ignore]
    fn gamepad_appears_as_xbox_pad() {
        let mut pad = Gamepad::new("test").unwrap();
        pad.update(State { buttons: pad::A | pad::UP, lx: 0.5, ry: -1.0, rt: 1.0, ..Default::default() }).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let devices = std::fs::read_to_string("/proc/bus/input/devices").unwrap();
        let entry = devices.split("\n\n").find(|d| d.contains("Tunnelrat gamepad (test)")).expect("device listed");
        assert!(entry.contains("Vendor=045e Product=028e"), "{entry}");
        assert!(entry.contains("js"), "joystick handler: {entry}");
        pad.update(State::default()).unwrap();
    }
}
