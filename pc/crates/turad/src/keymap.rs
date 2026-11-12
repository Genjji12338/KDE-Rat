//! Character → key strokes for the PC's *active* keyboard layout.
//!
//! uinput only sends key positions; the compositor applies the layout. So to
//! type "z" on a German layout we must press the key that says "Y" on a US
//! board. We compile the layout GNOME is using with libxkbcommon (loaded at
//! runtime) and, for each key under none/Shift/AltGr/Shift+AltGr, record the
//! character it produces. Characters only reachable via dead keys (e.g. `^`,
//! `é` on `de`) are typed as dead key + space / base letter.

use std::collections::HashMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use xkbcommon_dl::{xkb_context_flags, xkb_keymap_compile_flags, xkb_rule_names, xkbcommon_option};

pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTALT: u16 = 100; // AltGr / ISO_Level3_Shift on ISO layouts
pub const KEY_ENTER: u16 = 28;
pub const KEY_TAB: u16 = 15;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_SPACE: u16 = 57;

const SHIFT: u8 = 1;
const ALTGR: u8 = 2;

/// One physical key press with the layout modifiers it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub key: u16,
    pub shift: bool,
    pub altgr: bool,
}

impl Press {
    fn new(key: u16, mods: u8) -> Self {
        Self { key, shift: mods & SHIFT != 0, altgr: mods & ALTGR != 0 }
    }
}

/// XKB keysyms of the dead keys we know how to compose with.
const DEAD_GRAVE: u32 = 0xfe50;
const DEAD_ACUTE: u32 = 0xfe51;
const DEAD_CIRCUMFLEX: u32 = 0xfe52;
const DEAD_TILDE: u32 = 0xfe53;

/// (character, dead key, base character). Base ' ' = the spacing accent itself.
const DEAD_COMPOSE: &[(char, u32, char)] = &[
    ('^', DEAD_CIRCUMFLEX, ' '), ('`', DEAD_GRAVE, ' '), ('´', DEAD_ACUTE, ' '), ('~', DEAD_TILDE, ' '),
    ('â', DEAD_CIRCUMFLEX, 'a'), ('ê', DEAD_CIRCUMFLEX, 'e'), ('î', DEAD_CIRCUMFLEX, 'i'), ('ô', DEAD_CIRCUMFLEX, 'o'), ('û', DEAD_CIRCUMFLEX, 'u'),
    ('Â', DEAD_CIRCUMFLEX, 'A'), ('Ê', DEAD_CIRCUMFLEX, 'E'), ('Î', DEAD_CIRCUMFLEX, 'I'), ('Ô', DEAD_CIRCUMFLEX, 'O'), ('Û', DEAD_CIRCUMFLEX, 'U'),
    ('á', DEAD_ACUTE, 'a'), ('é', DEAD_ACUTE, 'e'), ('í', DEAD_ACUTE, 'i'), ('ó', DEAD_ACUTE, 'o'), ('ú', DEAD_ACUTE, 'u'), ('ý', DEAD_ACUTE, 'y'),
    ('Á', DEAD_ACUTE, 'A'), ('É', DEAD_ACUTE, 'E'), ('Í', DEAD_ACUTE, 'I'), ('Ó', DEAD_ACUTE, 'O'), ('Ú', DEAD_ACUTE, 'U'), ('Ý', DEAD_ACUTE, 'Y'),
    ('à', DEAD_GRAVE, 'a'), ('è', DEAD_GRAVE, 'e'), ('ì', DEAD_GRAVE, 'i'), ('ò', DEAD_GRAVE, 'o'), ('ù', DEAD_GRAVE, 'u'),
    ('À', DEAD_GRAVE, 'A'), ('È', DEAD_GRAVE, 'E'), ('Ì', DEAD_GRAVE, 'I'), ('Ò', DEAD_GRAVE, 'O'), ('Ù', DEAD_GRAVE, 'U'),
    ('ñ', DEAD_TILDE, 'n'), ('Ñ', DEAD_TILDE, 'N'), ('ã', DEAD_TILDE, 'a'), ('õ', DEAD_TILDE, 'o'),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutName {
    pub layout: String,
    pub variant: String,
    pub options: String,
}

pub struct Keymap {
    pub name: LayoutName,
    direct: HashMap<char, Press>,
    dead: HashMap<u32, Press>,
}

impl Keymap {
    pub fn compile(name: LayoutName) -> Result<Self> {
        let xkb = xkbcommon_option().context("libxkbcommon.so.0 not found")?;
        let c = |s: &str| CString::new(s).unwrap_or_default();
        let (layout, variant, options) = (c(&name.layout), c(&name.variant), c(&name.options));
        let (rules, model) = (c("evdev"), c("pc105"));

        // SAFETY: plain libxkbcommon C API; every object is freed before return
        // and no pointer escapes this function.
        unsafe {
            let ctx = (xkb.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS);
            if ctx.is_null() {
                bail!("xkb_context_new failed");
            }
            let names = xkb_rule_names {
                rules: rules.as_ptr(),
                model: model.as_ptr(),
                layout: layout.as_ptr(),
                variant: variant.as_ptr(),
                options: options.as_ptr(),
            };
            let km = (xkb.xkb_keymap_new_from_names)(ctx, &names, xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS);
            if km.is_null() {
                (xkb.xkb_context_unref)(ctx);
                bail!("could not compile keyboard layout {name:?}");
            }
            let shift_idx = (xkb.xkb_keymap_mod_get_index)(km, c("Shift").as_ptr());
            let l3_idx = (xkb.xkb_keymap_mod_get_index)(km, c("Mod5").as_ptr());
            let bit = |i: u32| if i == xkbcommon_dl::XKB_MOD_INVALID { 0 } else { 1u32 << i };

            let (min, max) = ((xkb.xkb_keymap_min_keycode)(km), (xkb.xkb_keymap_max_keycode)(km));
            let mut direct = HashMap::new();
            let mut dead = HashMap::new();
            // Fewest modifiers first so e.g. '1' maps to the plain key.
            for mods in [0u8, SHIFT, ALTGR, SHIFT | ALTGR] {
                let mask = (if mods & SHIFT != 0 { bit(shift_idx) } else { 0 }) | (if mods & ALTGR != 0 { bit(l3_idx) } else { 0 });
                let st = (xkb.xkb_state_new)(km);
                (xkb.xkb_state_update_mask)(st, mask, 0, 0, 0, 0, 0);
                for kc in min..=max {
                    // xkb keycode = evdev keycode + 8; ignore keys beyond what uinput registers.
                    let Some(evdev) = kc.checked_sub(8).filter(|k| (1..=248).contains(k)) else { continue };
                    let sym = (xkb.xkb_state_key_get_one_sym)(st, kc);
                    if (0xfe50..=0xfe53).contains(&sym) {
                        dead.entry(sym).or_insert(Press::new(evdev as u16, mods));
                        continue;
                    }
                    let u = (xkb.xkb_state_key_get_utf32)(st, kc);
                    if let Some(ch) = char::from_u32(u).filter(|c| !c.is_control()) {
                        direct.entry(ch).or_insert(Press::new(evdev as u16, mods));
                    }
                }
                (xkb.xkb_state_unref)(st);
            }
            (xkb.xkb_keymap_unref)(km);
            (xkb.xkb_context_unref)(ctx);
            if direct.is_empty() {
                bail!("layout {name:?} produced no characters");
            }
            Ok(Self { name, direct, dead })
        }
    }

    /// Key presses that type `ch`, or `None` if this layout can't produce it.
    pub fn strokes(&self, ch: char) -> Option<Vec<Press>> {
        let plain = |k| Press { key: k, shift: false, altgr: false };
        match ch {
            '\n' | '\r' => return Some(vec![plain(KEY_ENTER)]),
            '\t' => return Some(vec![plain(KEY_TAB)]),
            '\u{8}' => return Some(vec![plain(KEY_BACKSPACE)]),
            _ => {}
        }
        if let Some(p) = self.direct.get(&ch) {
            return Some(vec![*p]);
        }
        let &(_, dead_sym, base) = DEAD_COMPOSE.iter().find(|(c, _, _)| *c == ch)?;
        let dead = *self.dead.get(&dead_sym)?;
        let base = if base == ' ' { plain(KEY_SPACE) } else { *self.direct.get(&base)? };
        Some(vec![dead, base])
    }
}

/// Reads GNOME's current input source (most recently used first) and caches
/// the compiled keymap, re-checking at most every few seconds.
pub struct ActiveKeymap {
    current: Option<Keymap>,
    checked: Option<Instant>,
}

const RECHECK: Duration = Duration::from_secs(5);

impl ActiveKeymap {
    pub fn new() -> Self {
        Self { current: None, checked: None }
    }

    pub fn get(&mut self) -> Result<&Keymap> {
        if self.current.is_none() || self.checked.is_none_or(|t| t.elapsed() >= RECHECK) {
            self.checked = Some(Instant::now());
            let name = gnome_layout().unwrap_or(LayoutName { layout: "us".into(), variant: String::new(), options: String::new() });
            if self.current.as_ref().is_none_or(|k| k.name != name) {
                tracing::info!(layout = %name.layout, variant = %name.variant, "keyboard layout loaded");
                self.current = Some(Keymap::compile(name)?);
            }
        }
        Ok(self.current.as_ref().expect("set above"))
    }
}

fn gsettings(key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.input-sources", key]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// First `('xkb', 'de+nodeadkeys')` entry → layout "de", variant "nodeadkeys".
fn first_xkb_source(s: &str) -> Option<(String, String)> {
    let rest = &s[s.find("('xkb', '")? + "('xkb', '".len()..];
    let id = &rest[..rest.find('\'')?];
    let (layout, variant) = id.split_once('+').unwrap_or((id, ""));
    Some((layout.to_owned(), variant.to_owned()))
}

fn gnome_layout() -> Option<LayoutName> {
    let (layout, variant) = gsettings("mru-sources")
        .as_deref()
        .and_then(first_xkb_source)
        .or_else(|| gsettings("sources").as_deref().and_then(first_xkb_source))?;
    let options = gsettings("xkb-options")
        .map(|o| o.split('\'').skip(1).step_by(2).collect::<Vec<_>>().join(","))
        .unwrap_or_default();
    Some(LayoutName { layout, variant, options })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn km(layout: &str) -> Option<Keymap> {
        // Skips (passes) on machines without libxkbcommon/xkb-data.
        Keymap::compile(LayoutName { layout: layout.into(), variant: String::new(), options: String::new() }).ok()
    }

    #[test]
    fn parses_gsettings_sources() {
        assert_eq!(first_xkb_source("[('xkb', 'de')]"), Some(("de".into(), "".into())));
        assert_eq!(first_xkb_source("[('xkb', 'de+nodeadkeys'), ('xkb', 'us')]"), Some(("de".into(), "nodeadkeys".into())));
        assert_eq!(first_xkb_source("@a(ss) []"), None);
    }

    #[test]
    fn german_layout_maps_correctly() {
        let Some(de) = km("de") else { return };
        eprintln!("de: {} direct chars, {} dead keys", de.direct.len(), de.dead.len());
        let p = |c| de.strokes(c).unwrap();
        // QWERTZ: 'z' sits where US has 'y' (KEY_Y = 21), and vice versa.
        assert_eq!(p('z'), vec![Press { key: 21, shift: false, altgr: false }]);
        assert_eq!(p('y'), vec![Press { key: 44, shift: false, altgr: false }]);
        assert_eq!(p('Z'), vec![Press { key: 21, shift: true, altgr: false }]);
        // '@' = AltGr+Q on de.
        assert_eq!(p('@'), vec![Press { key: 16, shift: false, altgr: true }]);
        assert_eq!(p('ß').len(), 1);
        assert_eq!(p('ü').len(), 1);
        // Dead keys: '^' = dead_circumflex + space, 'é' = dead_acute + e.
        assert_eq!(p('^').len(), 2);
        assert_eq!(p('^')[1].key, KEY_SPACE);
        assert_eq!(p('é')[1], p('e')[0]);
        assert_eq!(p('\n'), vec![Press { key: KEY_ENTER, shift: false, altgr: false }]);
        assert!(de.strokes('😀').is_none());
    }

    #[test]
    fn us_layout_has_no_dead_keys_for_caret() {
        let Some(us) = km("us") else { return };
        assert_eq!(us.strokes('^').unwrap(), vec![Press { key: 7, shift: true, altgr: false }]); // Shift+6
        assert_eq!(us.strokes('z').unwrap()[0].key, 44);
    }
}
