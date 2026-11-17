//! Tunnelrat wire protocol.
//!
//! Each message is a single JSON object sent as one WebSocket text frame over
//! mutual TLS. The `type` field selects the variant. The Android app mirrors
//! these types in `android/app/src/main/java/dev/tunnelrat/proto/`.

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change to [`Message`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Default TCP port the PC daemon listens on.
pub const DEFAULT_PORT: u16 = 47_470;

/// Raw bytes per [`Message::FileChunk`].
pub const FILE_CHUNK: usize = 192 * 1024;

/// Largest file the PC accepts from a phone.
pub const MAX_FILE: u64 = 4 * 1024 * 1024 * 1024;

/// mDNS service type the PC daemon announces.
pub const MDNS_SERVICE: &str = "_tunnelrat._tcp.local.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// First message from each side after the TLS handshake.
    Hello {
        protocol: u32,
        device_name: String,
        device_kind: DeviceKind,
    },
    /// Sent by the PC so the phone can wake it later.
    PcInfo {
        hostname: String,
        /// MAC addresses of wake-capable interfaces, e.g. "aa:bb:cc:dd:ee:ff".
        wol_macs: Vec<String>,
        /// Power actions this PC allows. The phone hides the rest.
        #[serde(default)]
        power_actions: Vec<PowerAction>,
        /// False while the PC sits at the login screen (no desktop session):
        /// the phone then offers to log in. Re-sent when it changes.
        #[serde(default = "yes")]
        desktop: bool,
        /// Bluetooth adapter address ("AA:BB:..."), when the PC also accepts
        /// connections over Bluetooth (fallback without a network).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bluetooth: Option<String>,
    },
    /// Phone → PC at the login screen: type this password to log in.
    Login { password: String },
    /// PC → phone: whether that started a desktop session.
    LoginResult {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    // ── Pairing ────────────────────────────────────────────
    /// Phone → PC, first message from an unpaired phone. `token` comes from
    /// the QR code; the phone's TLS client certificate is what gets trusted.
    Pair { token: String, device_name: String },
    /// PC → phone: the phone's certificate is now trusted.
    PairOk { pc_name: String },
    /// Local CLI → PC (admin socket only): start a pairing window.
    PairBegin,
    /// PC → local CLI: the `tunnelrat://pair?...` URI to show as a QR code.
    /// `qr`: the code's modules, one string of '1'/'0' per row.
    PairOffer {
        uri: String,
        expires_in_secs: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        qr: Vec<String>,
    },
    /// Local CLI → PC: list paired phones.
    ListDevices,
    /// PC → local CLI. `peer`: the PC this one joined (`tura join`), if any.
    Devices {
        devices: Vec<DeviceInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer: Option<DeviceInfo>,
    },
    /// Local CLI → PC: revoke a phone by name (case-insensitive) or by
    /// fingerprint prefix (≥ 4 hex chars). Live sessions are dropped at once.
    Unpair { selector: String },
    /// PC → local CLI: the phone that was revoked.
    Unpaired { device: DeviceInfo },
    /// Phone → PC: "forget me" — the phone unpaired itself.
    ForgetMe,

    // ── Input ──────────────────────────────────────────────
    PointerMove { dx: f32, dy: f32 },
    PointerButton { button: MouseButton, state: PressState },
    Scroll { dx: f32, dy: f32 },
    /// Linux evdev key code (KEY_*), so special keys map one to one.
    /// `modifiers` are held around a `Click` (e.g. Ctrl+Tab).
    Key {
        code: u16,
        state: PressState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        modifiers: Vec<Modifier>,
    },
    /// Literal text typed into the focused window, mapped through the PC's
    /// active keyboard layout. `\n` = Enter, `\t` = Tab, `\u{8}` = Backspace.
    /// `modifiers` are held for every character (e.g. Ctrl + "c").
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        modifiers: Vec<Modifier>,
    },

    // ── Power ──────────────────────────────────────────────
    Power { action: PowerAction },

    // ── Media ──────────────────────────────────────────────
    /// Phone → PC: control the active media player (MPRIS).
    Media { action: MediaAction },
    /// Phone → PC: change system output volume by `step` percent (±).
    VolumeStep { step: i32 },
    /// Phone → PC: toggle system output mute.
    MuteToggle,
    /// PC → phone: sent on connect and whenever anything changes.
    NowPlaying(NowPlaying),

    // ── Clipboard ──────────────────────────────────────────
    /// Either direction: the sender's clipboard now holds `text`.
    Clipboard { text: String },

    // ── Phone status ───────────────────────────────────────
    /// Phone → PC, on connect and whenever it changes.
    Battery { level: u8, charging: bool },
    /// Phone → PC: a notification was posted (or updated) on the phone.
    /// `can_reply`: it has a reply field (messengers); see [`Message::PhoneReply`].
    PhoneNotification {
        key: String,
        app: String,
        title: String,
        text: String,
        #[serde(default)]
        can_reply: bool,
    },
    /// PC → phone: answer notification `key` through its own reply field.
    PhoneReply { key: String, text: String },
    /// Phone → PC: that notification is gone on the phone.
    PhoneNotificationRemoved { key: String },

    // ── Share to PC ────────────────────────────────────────
    /// Phone → PC: open an http(s) link in the PC's browser.
    OpenUrl { url: String },
    /// Phone → PC: a file transfer starts; followed by chunks and `FileEnd`.
    FileStart { id: u32, name: String, size: u64 },
    /// Base64 data, at most [`FILE_CHUNK`] raw bytes per message.
    FileChunk { id: u32, data: String },
    FileEnd { id: u32 },
    /// PC → phone: the file was saved (`path` on the PC).
    FileSaved { id: u32, path: String },

    // ── Phone as drive ─────────────────────────────────────
    /// Phone → PC: the phone shares files under `label` (mount it), or stops
    /// sharing (`label: None`, unmount).
    DriveOffer { label: Option<String> },
    /// PC → phone: one file-system call; answered by `FsReply` with the same id.
    FsRequest { id: u64, op: FsOp },
    FsReply { id: u64, result: FsResult },

    // ── PC ↔ PC ────────────────────────────────────────────
    /// Either PC → the other: its Deskflow (shared mouse/keyboard) identity.
    /// The server announces its port; each side trusts the other's
    /// certificate fingerprint (`v2:sha256:` hex, Deskflow's own format).
    DeskflowHello { role: DeskflowRole, screen: String, fingerprint: String, port: u16 },
    /// Local CLI → own turad: join another PC using its `tura pair` code.
    JoinPc { uri: String },
    /// Local CLI → own turad: deliver `message` to a connected device
    /// (a paired phone/PC, or the PC this one has joined).
    ForDevice { device: String, message: Box<Message> },
    /// Local CLI → own turad: ask for input capture again (shared mouse/keyboard)
    /// after the dialog was declined.
    ShareInput,
    /// Local app → own turad: read / change turad's settings.
    GetSettings,
    Settings { settings: TuradSettings },
    SetSettings { settings: TuradSettings },
    /// Local app → own turad: saved shared-mouse layout (see `SetLayout`).
    GetLayouts,
    Layouts { entries: Vec<LayoutEntry> },
    /// Local app → own turad: health checklist.
    GetStatus,
    Status { items: Vec<StatusItem> },
    /// Local CLI → own turad: where a joined PC sits for the shared mouse.
    SetLayout { device: String, side: Side, from: u8, to: u8 },

    // ── Screen (another device's screen on this PC) ──
    /// Local CLI/app → own turad: show `device`'s screen here. A PC is asked
    /// to cast (`CastStart`) and shown in a viewer window; a phone is asked to
    /// mirror (`MirrorRequest`).
    ScreenView { device: String },

    // ── Cast (a PC's screen → a phone or another PC) ──────
    /// Phone or PC → PC: stream your screen, scaled to fit `max_width`×`max_height`.
    CastStart { max_width: u32, max_height: u32 },
    /// PC → viewer: H.264 video of `width`×`height` follows as
    /// [`bin::VIDEO`] frames (one Annex B access unit each, id = sequence).
    CastStarted { width: u32, height: u32 },
    /// Viewer → PC: stop streaming.
    CastStop,
    /// Phone → PC (over the network link): let this phone pair over Bluetooth
    /// for a minute, so the Bluetooth fallback works. `name` is the phone's
    /// Bluetooth name; only a device with that name is accepted.
    BtPairStart { name: String },
    /// PC → phone: the PC is now visible and accepts the phone; bond now.
    BtPairReady,
    /// PC → phone: how Bluetooth pairing ended.
    BtPairResult {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// PC → viewer: the stream ended (`reason` if it wasn't asked to).
    CastEnded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Viewer → PC while casting: put the pointer at (x, y) within the shared
    /// screen, each 0.0–1.0 from the top-left corner.
    PointerAbs { x: f32, y: f32 },

    // ── Mirror (phone screen → PC) ─────────────────────────
    /// Phone → PC: the phone's screen follows as [`bin::VIDEO`] frames
    /// (`width`×`height`, H.264); the PC opens a viewer window.
    MirrorStart { width: u32, height: u32 },
    /// Either side: mirroring ended (phone stopped, or the viewer was closed).
    MirrorStop,
    /// PC → phone: please start mirroring (sent with `ForDevice` from `tura mirror`).
    MirrorRequest,
    /// Phone → PC: gamepad mode, the whole controller state (sent on every
    /// change). Buttons are [`pad`] bits; sticks −1…1 (y down), triggers 0…1.
    Gamepad { buttons: u32, lx: f32, ly: f32, rx: f32, ry: f32, lt: f32, rt: f32 },
    /// Phone → PC: gamepad mode closed; unplug the virtual controller.
    GamepadEnd,
    /// PC → phone: ring loudly so it can be found (even on silent), until
    /// stopped on the phone or after a while.
    Ring,
    /// PC → phone: perform a touch gesture. `path` points are
    /// `[x, y, t]`: x/y 0.0–1.0 of the screen, t milliseconds from the start.
    PhoneGesture { path: Vec<[f32; 3]> },
    /// PC → phone: a live touch (mouse held on the viewer): `down`, then
    /// `move`s, then `up`. x/y 0.0–1.0 of the screen.
    PhoneTouch { action: TouchAction, x: f32, y: f32 },
    /// PC → phone: a system navigation action.
    PhoneNav { action: PhoneNavAction },
    /// PC → phone: type into the focused field. `\n` = Enter, `\u{8}` = Backspace.
    PhoneText { text: String },

    // ── Generic ────────────────────────────────────────────
    Ping,
    Pong,
    Error { message: String },
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchAction {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneNavAction {
    Back,
    Home,
    Recents,
    Notifications,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeskflowRole {
    Server,
    Client,
}

/// Which edge of this PC's screen leads to the other one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Up => "up",
            Side::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    /// The Windows/Super key.
    Super,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NowPlaying {
    /// Player name, e.g. "Spotify"; `None` when no player is running.
    pub player: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    /// Cover art: an `https://` URL or a `data:image/...;base64,` URI.
    pub art: Option<String>,
    pub can_next: bool,
    pub can_previous: bool,
    /// System output volume in percent (may exceed 100 if boosted).
    pub volume: u32,
    pub muted: bool,
}

/// File-system operations, with `/`-separated paths relative to the shared
/// root ("" = the root). The phone must refuse paths that escape the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FsOp {
    Stat { path: String },
    List { path: String },
    Read { path: String, offset: u64, len: u32 },
    /// `data` is base64.
    Write { path: String, offset: u64, data: String },
    /// Creates an empty file (fails if it exists).
    Create { path: String },
    Mkdir { path: String },
    /// Deletes a file.
    Remove { path: String },
    /// Deletes an empty directory.
    Rmdir { path: String },
    Rename { from: String, to: String },
    Truncate { path: String, size: u64 },
    StatFs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    pub dir: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub mtime_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub stat: FileStat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsError {
    NotFound,
    Exists,
    NotEmpty,
    NotDir,
    IsDir,
    Denied,
    NoSpace,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsResult {
    Stat { stat: FileStat },
    List { entries: Vec<DirEntry> },
    /// Base64; shorter than requested at end of file.
    Data { data: String },
    Done,
    StatFs { total: u64, free: u64 },
    Error { error: FsError },
}

/// Gamepad button bits (`Message::Gamepad::buttons`).
pub mod pad {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LB: u32 = 1 << 4;
    pub const RB: u32 = 1 << 5;
    pub const VIEW: u32 = 1 << 6;
    pub const MENU: u32 = 1 << 7;
    pub const HOME: u32 = 1 << 8;
    pub const L3: u32 = 1 << 9;
    pub const R3: u32 = 1 << 10;
    pub const UP: u32 = 1 << 11;
    pub const DOWN: u32 = 1 << 12;
    pub const LEFT: u32 = 1 << 13;
    pub const RIGHT: u32 = 1 << 14;
}

/// Largest read/write chunk per request (raw bytes).
pub const FS_CHUNK: u32 = 1024 * 1024;

/// turad's own settings (`~/.config/tunnelrat/settings.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuradSettings {
    /// Sync the clipboard with phones and joined PCs.
    pub clipboard: bool,
    /// Offer suspend (some GPUs never wake from it; `--allow-suspend` forces it on).
    pub allow_suspend: bool,
    /// Shared mouse/keyboard (Deskflow) with joined PCs.
    pub shared_input: bool,
    /// Device fingerprints whose chip is hidden from the top bar.
    pub hidden_chips: Vec<String>,
    /// Phones (fingerprints): lock this PC when that phone has been gone for
    /// a short while (out of Wi-Fi and Bluetooth range, or switched off).
    pub lock_when_gone: Vec<String>,
}

impl Default for TuradSettings {
    fn default() -> Self {
        Self { clipboard: true, allow_suspend: false, shared_input: true, hidden_chips: vec![], lock_when_gone: vec![] }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutEntry {
    pub device: String,
    pub side: Side,
    pub from: u8,
    pub to: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusItem {
    pub name: String,
    pub ok: bool,
    /// What it means / how to fix it.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    /// SHA-256 hex of the phone's TLS client certificate.
    pub fingerprint: String,
    /// Unix seconds.
    pub paired_at: u64,
    /// Number of live connections right now.
    pub online: u32,
    /// Last reported battery percentage, if the phone sent one.
    #[serde(default)]
    pub battery: Option<u8>,
    #[serde(default)]
    pub charging: bool,
    /// Phone or PC (known once it has connected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DeviceKind>,
    /// Shown as a chip in the top bar (see [`TuradSettings::hidden_chips`]).
    #[serde(default = "yes")]
    pub in_top_bar: bool,
    /// Where its drive is mounted on this PC, if it's shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Phone,
    Pc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PressState {
    Down,
    Up,
    /// Down immediately followed by up.
    Click,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    Shutdown,
    Reboot,
    /// Off by default: suspend can hang some GPUs (see `turad --allow-suspend`).
    Suspend,
    Lock,
    /// Blank the displays; any input wakes them. Safe alternative to suspend.
    ScreenOff,
}

impl Message {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Message always serializes")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msgs = [
            Message::PointerMove { dx: 1.5, dy: -2.0 },
            Message::Power { action: PowerAction::Suspend },
            Message::Key { code: 30, state: PressState::Click, modifiers: vec![] },
            Message::Text { text: "c".into(), modifiers: vec![Modifier::Ctrl] },
            Message::Media { action: MediaAction::PlayPause },
            Message::FsRequest { id: 7, op: FsOp::Read { path: "DCIM/a.jpg".into(), offset: 0, len: 4096 } },
            Message::FsReply { id: 7, result: FsResult::Error { error: FsError::NotFound } },
            Message::NowPlaying(NowPlaying { player: Some("Spotify".into()), title: "T".into(), volume: 40, ..Default::default() }),
        ];
        for m in msgs {
            assert_eq!(Message::from_json(&m.to_json()).unwrap(), m);
        }
    }

    #[test]
    fn wire_format_is_stable() {
        let m = Message::Power { action: PowerAction::Shutdown };
        assert_eq!(m.to_json(), r#"{"type":"power","action":"shutdown"}"#);
        assert_eq!(Message::VolumeStep { step: -5 }.to_json(), r#"{"type":"volume_step","step":-5}"#);
        assert_eq!(
            Message::FsRequest { id: 1, op: FsOp::List { path: "".into() } }.to_json(),
            r#"{"type":"fs_request","id":1,"op":{"op":"list","path":""}}"#
        );
        assert_eq!(
            Message::FsReply { id: 1, result: FsResult::Done }.to_json(),
            r#"{"type":"fs_reply","id":1,"result":{"kind":"done"}}"#
        );
        let fd = Message::ForDevice { device: "x".into(), message: Box::new(Message::Power { action: PowerAction::Lock }) };
        assert_eq!(Message::from_json(&fd.to_json()).unwrap(), fd);
        // Older senders without `modifiers` still parse.
        assert_eq!(
            Message::from_json(r#"{"type":"text","text":"hi"}"#).unwrap(),
            Message::Text { text: "hi".into(), modifiers: vec![] }
        );
        assert!(Message::NowPlaying(NowPlaying::default()).to_json().starts_with(r#"{"type":"now_playing","player":null"#));
    }
}

/// Path of turad's local admin socket (plain WebSocket, owner-only).
pub fn admin_socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("tunnelrat.sock")
}

/// Binary WebSocket frames carry drive file data without JSON/base64 (which
/// is very slow on Android). Layout: `kind: u8`, `id: u64` (big-endian), then:
///
/// * [`BIN_READ_DATA`] (phone → PC): the bytes answering `FsOp::Read` `id`.
/// * [`BIN_WRITE`] (PC → phone): `path_len: u16`, `path` (UTF-8),
///   `offset: u64`, then the bytes. Answered by a JSON `FsReply`.
/// * [`VIDEO`]: one H.264 access unit of a screen cast or phone mirror.
pub mod bin {
    pub const READ_DATA: u8 = 1;
    pub const WRITE: u8 = 2;
    /// One H.264 access unit (Annex B); `id` = frame number. From a casting
    /// PC to its viewer, or from a mirroring phone to the PC.
    pub const VIDEO: u8 = 4;

    pub fn video(id: u64, data: &[u8]) -> Vec<u8> {
        frame(VIDEO, id, data)
    }

    fn frame(kind: u8, id: u64, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(9 + data.len());
        v.push(kind);
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    pub fn read_data(id: u64, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(9 + data.len());
        v.push(READ_DATA);
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    pub fn write(id: u64, path: &str, offset: u64, data: &[u8]) -> Vec<u8> {
        let p = path.as_bytes();
        let mut v = Vec::with_capacity(19 + p.len() + data.len());
        v.push(WRITE);
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(&(p.len() as u16).to_be_bytes());
        v.extend_from_slice(p);
        v.extend_from_slice(&offset.to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    /// `(kind, id, rest)` of any binary frame.
    pub fn split(frame: &[u8]) -> Option<(u8, u64, &[u8])> {
        let id = u64::from_be_bytes(frame.get(1..9)?.try_into().ok()?);
        Some((*frame.first()?, id, &frame[9..]))
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn roundtrip() {
            let f = super::read_data(42, b"abc");
            assert_eq!(super::split(&f), Some((super::READ_DATA, 42, &b"abc"[..])));
            let w = super::write(7, "DCIM/x.jpg", 1 << 33, b"zz");
            let (k, id, rest) = super::split(&w).unwrap();
            assert_eq!((k, id), (super::WRITE, 7));
            assert_eq!(u16::from_be_bytes([rest[0], rest[1]]), 10);
            assert_eq!(&rest[2..12], b"DCIM/x.jpg");
            assert_eq!(u64::from_be_bytes(rest[12..20].try_into().unwrap()), 1 << 33);
            assert_eq!(&rest[20..], b"zz");
            assert_eq!(super::split(&[1, 2]), None);
        }
    }
}
