//! PC screen → phone.
//!
//! A RemoteDesktop portal session with a ScreenCast stream: GNOME asks the
//! PC's user once which monitor to share (remembered via a restore token),
//! the stream carries the pointer drawn in, and the same session lets the
//! phone put the pointer at absolute positions. GStreamer (`gst-launch-1.0`,
//! hardware VA-API encoder when available, else x264) turns the PipeWire
//! stream into H.264; this module splits that into access units for the phone.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{info, warn};
use zbus::{Connection, MatchRule, MessageStream, OwnedObjectPath, OwnedVariant, Variant};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const RD: &str = "org.freedesktop.portal.RemoteDesktop";
const SC: &str = "org.freedesktop.portal.ScreenCast";
/// Largest size we encode, whatever the phone asks for.
const MAX_W: u32 = 2560;
const MAX_H: u32 = 1600;
/// Frames waiting to be sent; beyond this the phone/network is too slow and
/// we skip ahead to the next keyframe.
const QUEUE: usize = 4;
/// End of an access unit when no more bytes follow this quickly.
const AU_IDLE: Duration = Duration::from_millis(4);

/// Where the stream comes from.
enum Source {
    /// GNOME's own screen-cast/remote-desktop API (what GNOME Remote Desktop
    /// uses): no dialog. GNOME still shows its "screen is shared" indicator.
    Mutter { session: OwnedObjectPath, stream: OwnedObjectPath },
    /// The xdg portal (other desktops): asks the user, remembers the choice.
    Portal { session: OwnedObjectPath, node: u32 },
}

const M_RD: &str = "org.gnome.Mutter.RemoteDesktop";
const M_RD_SESSION: &str = "org.gnome.Mutter.RemoteDesktop.Session";
const M_SC: &str = "org.gnome.Mutter.ScreenCast";

pub struct Cast {
    /// Our own bus connection: if turad dies, GNOME ends the session.
    conn: Connection,
    source: Source,
    /// Stream size in GNOME's (logical) coordinates, for the pointer.
    logical: (f64, f64),
    /// Encoded video size.
    pub width: u32,
    pub height: u32,
    _encoder: Child,
    /// GNOME rejected an absolute position (logged once per cast).
    pointer_failed: std::sync::atomic::AtomicBool,
    /// Access units; taken by the session loop.
    pub frames: Option<mpsc::Receiver<Vec<u8>>>,
}

type Dict = HashMap<String, OwnedVariant>;

fn token() -> String {
    format!("tura{}", hex::encode(rand::random::<[u8; 6]>()))
}

fn token_path() -> std::path::PathBuf {
    crate::identity::config_dir().join("cast-token")
}

/// Calls a portal method that answers through a Request object's Response signal.
async fn request<B>(conn: &Connection, iface: &str, method: &str, body: &B, handle_token: &str) -> Result<Dict>
where
    B: serde::Serialize + zbus::DynamicType,
{
    let sender = conn.unique_name().context("no bus name")?.trim_start_matches(':').replace('.', "_");
    let path = format!("{PATH}/request/{sender}/{handle_token}");
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .member("Response")
        .path(path.as_str())
        .build()?;
    let mut responses = MessageStream::for_match_rule(rule, conn, None).await?;
    conn.call_method(Some(PORTAL), PATH, Some(iface), method, body).await.with_context(|| format!("{method}"))?;
    // The user may take a while to pick a screen.
    let msg = tokio::time::timeout(Duration::from_secs(120), responses.next())
        .await
        .context("nobody answered the screen-sharing question on the PC")?
        .context("session bus closed")??;
    let (code, results): (u32, Dict) = msg.body().deserialize()?;
    match code {
        0 => Ok(results),
        1 => bail!("screen sharing was declined on the PC"),
        _ => bail!("the PC's screen-sharing portal failed ({method})"),
    }
}

fn get_string(d: &Dict, key: &str) -> Option<String> {
    d.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

impl Cast {
    /// Starts sharing the primary screen: through GNOME directly (no dialog),
    /// else through the portal (may wait for the PC's user to pick one).
    pub async fn start(max_w: u32, max_h: u32) -> Result<Cast> {
        let conn = Connection::session().await?;
        let (source, node, (lw, lh), fd) = match mutter_source(&conn).await {
            Ok((source, node, size)) => (source, node, size, None),
            Err(e) => {
                info!("GNOME screen cast API unavailable ({e:#}); using the portal");
                let (source, node, size, fd) = portal_source(&conn).await?;
                (source, node, size, Some(fd))
            }
        };
        let logical = (f64::from(lw.max(1)), f64::from(lh.max(1)));
        let (width, height) = fit(lw.max(1) as u32, lh.max(1) as u32, max_w.clamp(320, MAX_W), max_h.clamp(240, MAX_H));
        let mut encoder = spawn_encoder(fd.as_ref(), node, width, height).await?;
        drop(fd); // the encoder has its own copy
        let stdout = encoder.stdout.take().context("encoder stdout")?;
        if let Some(err) = encoder.stderr.take() {
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    warn!("gstreamer: {l}");
                }
            });
        }
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(split_access_units(stdout, tx));
        info!(node, width, height, "casting the screen");
        Ok(Cast { conn, source, logical, width, height, _encoder: encoder, pointer_failed: Default::default(), frames: Some(rx) })
    }

    /// Puts the pointer at (x, y), each 0–1 within the shared screen.
    pub async fn pointer(&self, x: f32, y: f32) -> Result<()> {
        // Stay strictly inside: GNOME rejects x == width.
        let (x, y) = (f64::from(x.clamp(0.0, 0.999)) * self.logical.0, f64::from(y.clamp(0.0, 0.999)) * self.logical.1);
        let res = match &self.source {
            Source::Mutter { session, stream } => {
                self.conn.call_method(Some(M_RD), session.as_str(), Some(M_RD_SESSION), "NotifyPointerMotionAbsolute", &(stream.as_str(), x, y)).await
            }
            Source::Portal { session, node } => {
                self.conn
                    .call_method(Some(PORTAL), PATH, Some(RD), "NotifyPointerMotionAbsolute", &(session, HashMap::<&str, Variant>::new(), *node, x, y))
                    .await
            }
        };
        if let Err(e) = res
            && !self.pointer_failed.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            warn!(x, y, size = ?self.logical, "absolute pointer rejected: {e}");
        }
        Ok(())
    }

    /// Pointer movement, buttons and scrolling from the viewer, sent through
    /// the same remote-desktop session as [`Cast::pointer`]: one virtual
    /// device, in order. (Mixing in turad's uinput mouse let a click arrive
    /// before the pointer had moved, and split drags across two devices.)
    /// Returns false for other messages.
    pub async fn input(&self, m: &tura_proto::Message) -> Result<bool> {
        use tura_proto::{Message, MouseButton, PressState};
        match *m {
            Message::PointerMove { dx, dy } => {
                let (dx, dy) = (f64::from(dx), f64::from(dy));
                match &self.source {
                    Source::Mutter { session, .. } => self.rd(session, "NotifyPointerMotionRelative", &(dx, dy)).await?,
                    Source::Portal { session, .. } => self.portal("NotifyPointerMotion", &(session, opts(), dx, dy)).await?,
                }
            }
            Message::PointerButton { button, state } => {
                // evdev codes
                let code: i32 = match button {
                    MouseButton::Left => 0x110,
                    MouseButton::Right => 0x111,
                    MouseButton::Middle => 0x112,
                };
                let presses: &[bool] = match state {
                    PressState::Down => &[true],
                    PressState::Up => &[false],
                    PressState::Click => &[true, false],
                };
                for &down in presses {
                    match &self.source {
                        Source::Mutter { session, .. } => self.rd(session, "NotifyPointerButton", &(code, down)).await?,
                        Source::Portal { session, .. } => self.portal("NotifyPointerButton", &(session, opts(), code, u32::from(down))).await?,
                    }
                }
            }
            Message::Scroll { dx, dy } => {
                // Smooth scrolling: 10 units = one wheel click (libinput's scale);
                // dy > 0 scrolls down, like the protocol.
                let (dx, dy) = (f64::from(dx) * 10.0, f64::from(dy) * 10.0);
                match &self.source {
                    // SOURCE_CONTINUOUS (1 << 3): smooth, no kinetic follow-up.
                    Source::Mutter { session, .. } => self.rd(session, "NotifyPointerAxis", &(dx, dy, 1u32 << 3)).await?,
                    Source::Portal { session, .. } => self.portal("NotifyPointerAxis", &(session, opts(), dx, dy, 0u32)).await?,
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    async fn rd<B>(&self, session: &OwnedObjectPath, method: &str, body: &B) -> Result<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn.call_method(Some(M_RD), session.as_str(), Some(M_RD_SESSION), method, body).await?;
        Ok(())
    }

    async fn portal<B>(&self, method: &str, body: &B) -> Result<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn.call_method(Some(PORTAL), PATH, Some(RD), method, body).await?;
        Ok(())
    }

    /// Ends the session; the encoder is killed when `self` drops.
    pub async fn stop(self) {
        let _ = match &self.source {
            Source::Mutter { session, .. } => self.conn.call_method(Some(M_RD), session.as_str(), Some(M_RD_SESSION), "Stop", &()).await,
            Source::Portal { session, .. } => self.conn.call_method(Some(PORTAL), session.as_str(), Some("org.freedesktop.portal.Session"), "Close", &()).await,
        };
        info!("cast stopped");
    }
}

fn opts() -> HashMap<&'static str, Variant<'static>> {
    HashMap::new()
}

/// A D-Bus property as an owned variant.
async fn property(conn: &Connection, dest: &str, path: &str, iface: &str, name: &str) -> Result<OwnedVariant> {
    let reply = conn.call_method(Some(dest), path, Some("org.freedesktop.DBus.Properties"), "Get", &(iface, name)).await?;
    Ok(reply.body().deserialize::<OwnedVariant>()?)
}

/// The primary monitor's connector name (e.g. "DP-1"), from Mutter's display config.
async fn primary_connector(conn: &Connection) -> Result<String> {
    type Spec = (String, String, String, String);
    type Mode = (String, i32, i32, f64, f64, Vec<f64>, Dict);
    type Logical = (i32, i32, f64, u32, bool, Vec<Spec>, Dict);
    let reply = conn
        .call_method(Some("org.gnome.Mutter.DisplayConfig"), "/org/gnome/Mutter/DisplayConfig", Some("org.gnome.Mutter.DisplayConfig"), "GetCurrentState", &())
        .await?;
    let (_serial, _monitors, logical, _props): (u32, Vec<(Spec, Vec<Mode>, Dict)>, Vec<Logical>, Dict) = reply.body().deserialize()?;
    logical.iter().find(|l| l.4).or(logical.first()).and_then(|l| l.5.first()).map(|m| m.0.clone()).context("no monitor")
}

/// A Mutter remote-desktop session recording the primary monitor (pointer drawn in).
async fn mutter_source(conn: &Connection) -> Result<(Source, u32, (i32, i32))> {
    let session: OwnedObjectPath = conn.call_method(Some(M_RD), "/org/gnome/Mutter/RemoteDesktop", Some(M_RD), "CreateSession", &()).await?.body().deserialize()?;
    let id = String::try_from(property(conn, M_RD, session.as_str(), M_RD_SESSION, "SessionId").await?)?;
    let opts: HashMap<&str, Variant> = HashMap::from([("remote-desktop-session-id", Variant::from(id.as_str()))]);
    let sc: OwnedObjectPath = conn.call_method(Some(M_SC), "/org/gnome/Mutter/ScreenCast", Some(M_SC), "CreateSession", &(opts,)).await?.body().deserialize()?;
    let connector = primary_connector(conn).await.unwrap_or_default();
    let opts: HashMap<&str, Variant> = HashMap::from([("cursor-mode", Variant::from(1u32))]); // embedded
    let stream: OwnedObjectPath = conn
        .call_method(Some(M_SC), sc.as_str(), Some("org.gnome.Mutter.ScreenCast.Session"), "RecordMonitor", &(connector.as_str(), opts))
        .await
        .context("RecordMonitor")?
        .body()
        .deserialize()?;
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.gnome.Mutter.ScreenCast.Stream")
        .member("PipeWireStreamAdded")
        .path(stream.as_str())
        .build()?;
    let mut added = MessageStream::for_match_rule(rule, conn, None).await?;
    conn.call_method(Some(M_RD), session.as_str(), Some(M_RD_SESSION), "Start", &()).await.context("Start")?;
    let msg = tokio::time::timeout(Duration::from_secs(10), added.next()).await.context("GNOME didn't start the stream")?.context("bus closed")??;
    let (node,): (u32,) = msg.body().deserialize()?;
    let params = property(conn, M_SC, stream.as_str(), "org.gnome.Mutter.ScreenCast.Stream", "Parameters").await?;
    let params: Dict = params.try_into().unwrap_or_default();
    let size: (i32, i32) = params.get("size").and_then(|v| v.try_clone().ok()?.try_into().ok()).unwrap_or((1920, 1080));
    info!(node, %connector, ?size, "GNOME screen cast");
    Ok((Source::Mutter { session, stream }, node, size))
}

/// A portal session (asks the user the first time; remembered via a restore token).
async fn portal_source(conn: &Connection) -> Result<(Source, u32, (i32, i32), OwnedFd)> {
    let conn = conn.clone();
    {
        let (t, st) = (token(), token());
        let opts: HashMap<&str, Variant> = HashMap::from([("handle_token", Variant::from(t.as_str())), ("session_handle_token", Variant::from(st.as_str()))]);
        let res = request(&conn, RD, "CreateSession", &(opts,), &t).await?;
        let session = OwnedObjectPath::try_from(get_string(&res, "session_handle").context("no session handle")?)?;

        let t = token();
        let restore = std::fs::read_to_string(token_path()).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let mut opts: HashMap<&str, Variant> = HashMap::from([
            ("handle_token", Variant::from(t.as_str())),
            ("types", Variant::from(2u32)), // pointer
            ("persist_mode", Variant::from(2u32)),
        ]);
        if let Some(r) = &restore {
            opts.insert("restore_token", Variant::from(r.as_str()));
        }
        request(&conn, RD, "SelectDevices", &(&session, opts), &t).await?;

        let t = token();
        let opts: HashMap<&str, Variant> = HashMap::from([
            ("handle_token", Variant::from(t.as_str())),
            ("types", Variant::from(1u32)), // a monitor
            ("multiple", Variant::from(false)),
            ("cursor_mode", Variant::from(2u32)), // pointer drawn into the picture
        ]);
        request(&conn, SC, "SelectSources", &(&session, opts), &t).await?;

        let t = token();
        let opts: HashMap<&str, Variant> = HashMap::from([("handle_token", Variant::from(t.as_str()))]);
        let res = request(&conn, RD, "Start", &(&session, "", opts), &t).await?;
        if let Some(tok) = get_string(&res, "restore_token") {
            let p = token_path();
            if std::fs::write(&p, tok).is_ok() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
            }
        }
        let streams: Vec<(u32, Dict)> = res
            .get("streams")
            .context("no screen was shared")?
            .try_clone()?
            .try_into()
            .context("unexpected stream list")?;
        let (node, props) = streams.into_iter().next().context("no screen was shared")?;
        let size: (i32, i32) = props.get("size").and_then(|v| v.try_clone().ok()?.try_into().ok()).unwrap_or((1920, 1080));
        info!(node, ?props, "portal stream");
        let reply = conn
            .call_method(Some(PORTAL), PATH, Some(SC), "OpenPipeWireRemote", &(&session, HashMap::<&str, Variant>::new()))
            .await
            .context("OpenPipeWireRemote")?;
        let fd: zbus::OwnedFd = reply.body().deserialize()?;
        Ok((Source::Portal { session, node }, node, size, fd.into()))
    }
}

/// `w`×`h` scaled to fit `max_w`×`max_h`, even dimensions. Scales up at most
/// 2× (the portal reports scaled-desktop sizes, smaller than the real pixels).
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let s = (f64::from(max_w) / f64::from(w)).min(f64::from(max_h) / f64::from(h)).min(2.0);
    let even = |v: f64| ((v as u32) & !1).max(2);
    (even(f64::from(w) * s), even(f64::from(h) * s))
}

pub async fn has_element(name: &str) -> bool {
    Command::new("gst-inspect-1.0").arg(name).stdout(Stdio::null()).stderr(Stdio::null()).status().await.is_ok_and(|s| s.success())
}

/// `fd`: the portal's PipeWire connection; `None` = this user's PipeWire.
async fn spawn_encoder(fd: Option<&OwnedFd>, node: u32, w: u32, h: u32) -> Result<Child> {
    // One argv word per pipeline token (caps contain parentheses, so no shell).
    fn add(args: &mut Vec<String>, s: &str) {
        args.extend(s.split(' ').map(str::to_owned));
    }
    let mut args: Vec<String> = vec![];
    let fd_arg = if fd.is_some() { "fd=3 " } else { "" };
    add(&mut args, &format!("-q pipewiresrc {fd_arg}path={node} do-timestamp=true keepalive-time=100 ! videorate max-rate=30 drop-only=true"));
    if has_element("vah264enc").await && has_element("vapostproc").await {
        add(&mut args, &format!("! videoconvert ! vapostproc ! video/x-raw(memory:VAMemory),format=NV12,width={w},height={h}"));
        add(&mut args, "! vah264enc b-frames=0 key-int-max=30 rate-control=cbr bitrate=8000 aud=true ! video/x-h264,profile=main,stream-format=byte-stream");
    } else if has_element("x264enc").await {
        add(&mut args, &format!("! videoconvert ! videoscale ! video/x-raw,format=I420,width={w},height={h}"));
        add(&mut args, "! x264enc tune=zerolatency speed-preset=veryfast bitrate=6000 key-int-max=30 aud=true byte-stream=true ! video/x-h264,profile=main,stream-format=byte-stream");
    } else {
        bail!("no H.264 encoder: sudo apt install gstreamer1.0-plugins-ugly (x264) or gstreamer1.0-plugins-bad (VA-API)");
    }
    add(&mut args, "! fdsink fd=1 sync=false");

    let mut cmd = Command::new("gst-launch-1.0");
    cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    if let Some(fd) = fd {
    let raw = fd.as_raw_fd();
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if raw == 3 {
                // Already fd 3: just let it survive exec.
                let flags = libc::fcntl(3, libc::F_GETFD);
                if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(raw, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    }
    cmd.spawn().context("starting gst-launch-1.0 (sudo apt install gstreamer1.0-tools gstreamer1.0-pipewire)")
}

/// Start offsets of the NAL units in Annex B data (at the 00 00 01 start code).
fn nal_starts(d: &[u8]) -> impl Iterator<Item = usize> + '_ {
    d.windows(3).enumerate().filter(|(_, w)| w == &[0, 0, 1]).map(|(i, _)| i)
}

/// Where the next access unit starts: an access-unit delimiter after offset 0.
fn next_aud(d: &[u8]) -> Option<usize> {
    nal_starts(d)
        .filter(|&i| d.get(i + 3).is_some_and(|b| b & 0x1f == 9))
        .map(|i| if i > 0 && d[i - 1] == 0 { i - 1 } else { i }) // 4-byte start code
        .find(|&i| i > 0)
}

pub fn is_keyframe(au: &[u8]) -> bool {
    nal_starts(au).any(|i| au.get(i + 3).is_some_and(|b| b & 0x1f == 5))
}

/// Reads the encoder's byte stream and queues whole access units; when the
/// queue is full, skips frames until the next keyframe (P-frames need their
/// predecessors).
async fn split_access_units(mut out: tokio::process::ChildStdout, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    let mut chunk = vec![0u8; 256 * 1024];
    let mut skipping = false;
    let emit = |au: Vec<u8>, skipping: &mut bool| -> bool {
        if *skipping && !is_keyframe(&au) {
            return true;
        }
        match tx.try_send(au) {
            Ok(()) => *skipping = false,
            Err(mpsc::error::TrySendError::Full(_)) => *skipping = true,
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
        }
        true
    };
    loop {
        let read = if buf.is_empty() {
            out.read(&mut chunk).await
        } else {
            match tokio::time::timeout(AU_IDLE, out.read(&mut chunk)).await {
                Ok(r) => r,
                Err(_) => {
                    // Quiet: what we have is a whole frame.
                    if !emit(std::mem::take(&mut buf), &mut skipping) {
                        return;
                    }
                    continue;
                }
            }
        };
        match read {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        while let Some(at) = next_aud(&buf) {
            let rest = buf.split_off(at);
            if !emit(std::mem::replace(&mut buf, rest), &mut skipping) {
                return;
            }
        }
    }
    if !buf.is_empty() {
        emit(buf, &mut skipping);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_and_splits() {
        assert_eq!(fit(2560, 1440, 2400, 1080), (1920, 1080));
        assert_eq!(fit(1536, 864, 2400, 1080), (1920, 1080));
        assert_eq!(fit(640, 360, 2400, 1080), (1280, 720));
        let aud = [0, 0, 0, 1, 9, 0xf0];
        let idr = [0, 0, 1, 0x65, 1, 2, 3];
        let p = [0, 0, 1, 0x41, 4, 5];
        let mut s = vec![];
        s.extend_from_slice(&aud);
        s.extend_from_slice(&idr);
        let second = s.len();
        s.extend_from_slice(&aud);
        s.extend_from_slice(&p);
        assert_eq!(next_aud(&s), Some(second));
        assert!(is_keyframe(&s[..second]));
        assert!(!is_keyframe(&s[second..]));
    }
}
