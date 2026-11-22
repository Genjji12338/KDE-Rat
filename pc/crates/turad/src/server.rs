//! Connections.
//!
//! * Phones: TCP + TLS with a **mandatory** client certificate. A phone whose
//!   cert fingerprint isn't paired may only send `Pair { token }` first;
//!   anything else closes the connection.
//! * Local CLI (`tura`): plain WebSocket over a Unix socket in
//!   `$XDG_RUNTIME_DIR` with mode 0600, so only this user can use it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::broadcast;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{info, warn};
use tura_proto::{DeviceInfo, DeviceKind, Message, Modifier, PROTOCOL_VERSION};

use crate::identity::{self, Identity};
use crate::pairing::{Pairing, TOKEN_TTL};
use crate::{input::Input, power, sysinfo};

/// The uinput device, opened lazily and re-tried.
///
/// At login the service can start before logind/udev grant the seat user its
/// ACL on /dev/uinput (uaccess), so a failed open is retried later instead of
/// disabling input for the daemon's lifetime.
struct LazyInput {
    dev: Option<Input>,
    last_try: Option<Instant>,
}

const INPUT_RETRY: Duration = Duration::from_secs(2);

impl LazyInput {
    fn get(&mut self) -> Result<&mut Input> {
        if self.dev.is_none() && self.last_try.is_none_or(|t| t.elapsed() >= INPUT_RETRY) {
            self.last_try = Some(Instant::now());
            match Input::new() {
                Ok(i) => {
                    info!("input device ready");
                    self.dev = Some(i);
                }
                Err(e) => warn!("input unavailable, will retry: {e:#}"),
            }
        }
        self.dev.as_mut().ok_or_else(|| anyhow::anyhow!("input injection unavailable on this PC (no access to /dev/uinput)"))
    }
}

pub(crate) struct Ctx {
    input: Mutex<LazyInput>,
    /// From the command line (`--allow-suspend`); see [`Ctx::policy`].
    cli_policy: power::Policy,
    pub(crate) settings: Mutex<tura_proto::TuradSettings>,
    /// Where each phone's drive is mounted (by fingerprint).
    drives: Mutex<HashMap<String, String>>,
    pairing: Pairing,
    pc_fingerprint: String,
    port: u16,
    /// Live connection count per phone fingerprint.
    online: Mutex<HashMap<String, u32>>,
    /// Fingerprints that were just unpaired; their sessions close at once.
    revoked: broadcast::Sender<String>,
    /// true = the PC is about to sleep (unmount drives), false = it woke up.
    sleep: broadcast::Sender<bool>,
    media: crate::media::Media,
    keymap: Mutex<crate::keymap::ActiveKeymap>,
    pub(crate) shell: crate::shell::Shell,
    pub(crate) notifier: Arc<crate::notify::Notifier>,
    /// Last battery report per phone fingerprint: (level, charging).
    battery: Mutex<HashMap<String, (u8, bool)>>,
    /// (phone fingerprint, phone notification key) → desktop notification id.
    phone_notifs: Mutex<HashMap<(String, String), u32>>,
    /// Folders where a phone is currently mounted (unmounted on shutdown).
    mounts: MountRegistry,
    /// Our TLS identity (also the client certificate when joining a PC).
    pub(crate) identity: Arc<Identity>,
    pub(crate) deskflow: crate::deskflow::Deskflow,
    layouts: Mutex<crate::deskflow::Layouts>,
    /// The PC this one has joined (`tura join`), if any.
    pub(crate) peer_cfg: tokio::sync::watch::Sender<Option<crate::peer::PeerConfig>>,
    /// Outgoing queue of the link to that PC, while connected: (its name, sender).
    pub(crate) peer_out: Mutex<Option<(String, tokio::sync::mpsc::UnboundedSender<WsMessage>)>>,
    /// Live device sessions by fingerprint: (name, outgoing queue).
    sessions: Mutex<HashMap<String, (String, tokio::sync::mpsc::UnboundedSender<WsMessage>)>>,
}

type MountRegistry = Arc<Mutex<std::collections::HashSet<std::path::PathBuf>>>;

/// Largest picture a PC viewer asks for (the cast scales down to fit).
const VIEW_MAX: (u32, u32) = (2560, 1600);

impl Ctx {
    /// Mouse/keyboard events from a viewer of our screen.
    pub(crate) fn inject(&self, m: &Message) -> Result<()> {
        let mut guard = self.input.lock().unwrap_or_else(|p| p.into_inner());
        let input = guard.get()?;
        match m {
            Message::PointerButton { button, state } => input.button(*button, *state),
            Message::Scroll { dx, dy } => input.scroll(*dx, *dy),
            Message::Key { code, state, .. } => input.key(*code, *state),
            Message::PointerMove { dx, dy } => input.pointer_move(*dx, *dy),
            _ => Ok(()),
        }
    }

    pub(crate) fn policy(&self) -> power::Policy {
        power::Policy { allow_suspend: self.cli_policy.allow_suspend || self.settings.lock().unwrap().allow_suspend }
    }

    pub(crate) fn clipboard_on(&self) -> bool {
        self.settings.lock().unwrap().clipboard
    }

    /// Shared handle for blocking mount/unmount threads.
    fn clone_mounts_handle(&self) -> MountRegistry {
        self.mounts.clone()
    }

    fn device_info(&self, d: &crate::pairing::PairedDevice) -> DeviceInfo {
        // Read each mutex in its own statement: a guard in a struct literal lives
        // until the literal ends, so locking the same mutex twice there deadlocks.
        let online = self.online.lock().unwrap().get(&d.fingerprint).copied().unwrap_or(0);
        let battery = self.battery.lock().unwrap().get(&d.fingerprint).copied();
        let in_top_bar = !self.settings.lock().unwrap().hidden_chips.contains(&d.fingerprint);
        let drive = self.drives.lock().unwrap().get(&d.fingerprint).cloned();
        DeviceInfo {
            kind: d.kind.as_deref().map(|k| if k == "pc" { DeviceKind::Pc } else { DeviceKind::Phone }),
            in_top_bar,
            drive,
            name: d.name.clone(),
            fingerprint: d.fingerprint.clone(),
            paired_at: d.paired_at,
            online,
            battery: battery.map(|b| b.0),
            charging: battery.is_some_and(|b| b.1),
        }
    }
}

/// Counts a phone as online for as long as it lives.
struct OnlineGuard<'a> {
    ctx: &'a Ctx,
    fingerprint: String,
}

impl<'a> OnlineGuard<'a> {
    fn new(ctx: &'a Ctx, fingerprint: &str) -> Self {
        *ctx.online.lock().unwrap().entry(fingerprint.to_owned()).or_default() += 1;
        Self { ctx, fingerprint: fingerprint.to_owned() }
    }
}

impl Drop for OnlineGuard<'_> {
    fn drop(&mut self) {
        let mut online = self.ctx.online.lock().unwrap();
        if let Some(n) = online.get_mut(&self.fingerprint) {
            *n -= 1;
            if *n == 0 {
                online.remove(&self.fingerprint);
            }
        }
    }
}

#[derive(Debug, Clone)]
enum Peer {
    /// Remote phone, identified by its client-cert fingerprint.
    /// `addr`: where it connects from (IP:port, or "Bluetooth <MAC>").
    Phone { addr: String, fingerprint: String },
    /// This user's own `tura` CLI over the admin socket.
    Local,
}

pub async fn run(listen: SocketAddr, policy: power::Policy) -> Result<()> {
    let dir = identity::config_dir();
    let id: Identity = identity::load_or_create(&dir, &sysinfo::hostname())?;
    info!(fingerprint = %id.fingerprint, "TLS identity loaded");
    let acceptor = TlsAcceptor::from(crate::tls::server_config(&id)?);
    let notifier = Arc::new(crate::notify::Notifier::new());

    let ctx = Arc::new(Ctx {
        input: Mutex::new(LazyInput { dev: None, last_try: None }),
        cli_policy: policy,
        settings: Mutex::new(crate::settings::load()),
        drives: Mutex::new(HashMap::new()),
        pairing: Pairing::load(dir.join("paired.json"))?,
        pc_fingerprint: id.fingerprint.clone(),
        port: listen.port(),
        online: Mutex::new(HashMap::new()),
        revoked: broadcast::channel(16).0,
        sleep: broadcast::channel(4).0,
        media: crate::media::Media::new(),
        keymap: Mutex::new(crate::keymap::ActiveKeymap::new()),
        shell: crate::shell::Shell::new(),
        notifier: notifier.clone(),
        battery: Mutex::new(HashMap::new()),
        phone_notifs: Mutex::new(HashMap::new()),
        mounts: Arc::new(Mutex::new(std::collections::HashSet::new())),
        identity: Arc::new(Identity { cert: id.cert.clone(), key: id.key.clone_key(), fingerprint: id.fingerprint.clone() }),
        deskflow: crate::deskflow::Deskflow::new(&dir, notifier.clone()),
        layouts: Mutex::new(crate::deskflow::Layouts::load(&dir)),
        peer_cfg: tokio::sync::watch::channel(crate::peer::PeerConfig::load()).0,
        peer_out: Mutex::new(None),
        sessions: Mutex::new(HashMap::new()),
    });
    {
        let c = ctx.clone();
        tokio::spawn(async move { c.deskflow.run().await });
        let c = ctx.clone();
        tokio::spawn(async move { c.deskflow.watch_session().await });
        let c = ctx.clone();
        tokio::spawn(crate::peer::run(c));
        tokio::spawn(crate::mdns::run(listen.port(), id.fingerprint.clone()));
        let mounts = ctx.clone_mounts_handle();
        tokio::spawn(crate::sleep::run(ctx.sleep.clone(), move || !mounts.lock().unwrap().is_empty()));
    }
    {
        let c = ctx.clone();
        tokio::spawn(async move { c.shell.run().await });
        let c = ctx.clone();
        tokio::spawn(async move { c.notifier.run_actions().await });
    }
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let online = || !ctx.online.lock().unwrap().is_empty();
            ctx.media.poll_loop(online).await;
        });
    }
    let _ = ctx.input.lock().unwrap().get(); // early log line
    {
        let s = ctx.settings.lock().unwrap().clone();
        ctx.deskflow.set_enabled(s.shared_input);
        let c = ctx.clone();
        tokio::spawn(async move { c.shell.set_hidden(&s.hidden_chips).await });
    }

    tokio::spawn(crate::bluetooth::run(acceptor.clone(), ctx.clone()));

    let tcp = TcpListener::bind(listen).await.with_context(|| format!("binding {listen}"))?;
    info!("listening for phones on {listen} (mTLS)");

    let sock_path = tura_proto::admin_socket_path();
    let _ = std::fs::remove_file(&sock_path);
    let unix = UnixListener::bind(&sock_path).with_context(|| format!("binding {}", sock_path.display()))?;
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))?;
    info!(path = %sock_path.display(), "admin socket ready");

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            _ = term.recv() => {
                info!("shutting down; unmounting phones");
                crate::drive::unmount_all(&ctx.mounts);
                return Ok(());
            }
            _ = tokio::signal::ctrl_c() => {
                crate::drive::unmount_all(&ctx.mounts);
                return Ok(());
            }
            res = tcp.accept() => {
                let (stream, addr) = res?;
                let (acceptor, ctx) = (acceptor.clone(), ctx.clone());
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    accept_phone(&acceptor, stream, addr.to_string(), ctx).await;
                });
            }
            res = unix.accept() => {
                let (stream, _) = res?;
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(e) = session(stream, Peer::Local, ctx).await {
                        warn!("local session ended: {e:#}");
                    }
                });
            }
        }
    }
}

/// How long a phone may stay gone before "lock when I leave" locks the PC.
/// Covers hopping between Wi-Fi and Bluetooth, and short Wi-Fi drops.
const GONE_GRACE: Duration = Duration::from_secs(30);

/// "Lock when I leave": `fp` just went offline; lock unless it comes back
/// (or the PC goes to sleep, which drops every phone) within [`GONE_GRACE`].
async fn lock_if_still_gone(ctx: Arc<Ctx>, fp: String, name: String) {
    let mut sleep = ctx.sleep.subscribe();
    let slept = tokio::time::timeout(GONE_GRACE, async {
        loop {
            match sleep.recv().await {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => return,
                Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
            }
        }
    })
    .await
    .is_ok();
    if slept || ctx.online.lock().unwrap().contains_key(&fp) || !ctx.settings.lock().unwrap().lock_when_gone.contains(&fp) {
        return;
    }
    if crate::deskflow::graphical_session().await.is_none() {
        return; // nobody logged in
    }
    match power::run(tura_proto::PowerAction::Lock, ctx.policy()).await {
        Ok(()) => {
            info!(%name, "phone gone: locked this PC");
            let _ = ctx.notifier.show("Tunnelrat", "system-lock-screen-symbolic", "Locked", &format!("{name} left, so this PC was locked."), 0).await;
        }
        Err(e) => warn!(%name, "phone gone, but locking failed: {e:#}"),
    }
}

/// TLS handshake with a phone or PC (over TCP or Bluetooth), then its session.
pub(crate) async fn accept_phone<S: AsyncRead + AsyncWrite + Unpin>(acceptor: &TlsAcceptor, stream: S, addr: String, ctx: Arc<Ctx>) {
    let tls = match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return warn!(%addr, "TLS handshake failed: {e}"),
        Err(_) => return warn!(%addr, "TLS handshake timed out"),
    };
    let Some(cert) = tls.get_ref().1.peer_certificates().and_then(|c| c.first()) else {
        return warn!(%addr, "no client certificate");
    };
    let peer = Peer::Phone { addr, fingerprint: identity::fingerprint(cert) };
    if let Err(e) = session(tls, peer.clone(), ctx).await {
        warn!(?peer, "session ended: {e:#}");
    }
}

fn frame(m: Message) -> WsMessage {
    WsMessage::Text(m.to_json().into())
}

/// What arrived on the socket. `Activity` = ping/pong/binary: proof of life only.
enum Incoming {
    Msg(Message),
    Binary(Vec<u8>),
    Activity,
    Closed,
}

async fn next_frame<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Result<Incoming> {
    match ws.next().await {
        None => Ok(Incoming::Closed),
        Some(f) => match f? {
            WsMessage::Text(t) => Ok(Incoming::Msg(Message::from_json(&t).context("bad message")?)),
            WsMessage::Close(_) => Ok(Incoming::Closed),
            WsMessage::Binary(b) => Ok(Incoming::Binary(b.to_vec())),
            _ => Ok(Incoming::Activity),
        },
    }
}

/// Phones ping every 15 s (OkHttp). A phone that changes network (new IP)
/// leaves its old TCP connection half-open; without this, turad would keep
/// writing into it and count the phone online for TCP's ~15 min retry window.
const PHONE_IDLE: Duration = Duration::from_secs(45);

async fn next_msg<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Result<Option<Message>> {
    while let Some(f) = ws.next().await {
        match f? {
            WsMessage::Text(t) => return Ok(Some(Message::from_json(&t).context("bad message")?)),
            WsMessage::Close(_) => return Ok(None),
            _ => continue,
        }
    }
    Ok(None)
}

async fn session<S: AsyncRead + AsyncWrite + Unpin>(stream: S, peer: Peer, ctx: Arc<Ctx>) -> Result<()> {
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    let mut revoked = ctx.revoked.subscribe();

    // ── Gate: unpaired devices must pair first ──
    if let Peer::Phone { addr, fingerprint } = &peer {
        match ctx.pairing.is_paired(fingerprint) {
            Some(name) => info!(%addr, %name, "paired device connected"),
            None => {
                let first = tokio::time::timeout(Duration::from_secs(10), next_msg(&mut ws)).await;
                let Ok(Ok(Some(Message::Pair { token, device_name }))) = first else {
                    let _ = ws.send(frame(Message::Error { message: "not paired: scan the QR code from `tura pair`".into() })).await;
                    let _ = ws.close(None).await;
                    bail!("unpaired device {addr} did not pair");
                };
                if !ctx.pairing.redeem(&token, fingerprint, &device_name)? {
                    let _ = ws.send(frame(Message::Error { message: "pairing code invalid or expired — run `tura pair` again".into() })).await;
                    let _ = ws.close(None).await;
                    bail!("bad pairing token from {addr}");
                }
                info!(%addr, %device_name, %fingerprint, "new device paired");
                ws.send(frame(Message::PairOk { pc_name: sysinfo::hostname() })).await?;
            }
        }
    }

    let my_fp = match &peer {
        Peer::Phone { fingerprint, .. } => Some(fingerprint.clone()),
        Peer::Local => None,
    };
    let _online = my_fp.as_deref().map(|fp| OnlineGuard::new(&ctx, fp));
    let mut media_rx = ctx.media.subscribe();
    let mut clip_rx = ctx.shell.clipboard_changes();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut st = SessionState {
        name: my_fp.as_deref().and_then(|fp| ctx.pairing.is_paired(fp)).unwrap_or_else(|| "Phone".into()),
        files: crate::files::Receiver::default(),
        gamepad: None,
        out_tx,
        drive: None,
        deskflow_screen: None,
        cast: None,
        cast_starting: None,
        cast_wanted: false,
        mirror: None,
        asleep_label: None,
        fp: my_fp.clone(),
        kind: "phone",
    };
    if let Some(fp) = &my_fp {
        ctx.sessions.lock().unwrap().insert(fp.clone(), (st.name.clone(), st.out_tx.clone()));
    }

    let result = session_loop(&mut ws, &peer, &ctx, &my_fp, &mut st, &mut media_rx, &mut clip_rx, &mut revoked, &mut out_rx).await;

    // ── Cleanup, whatever ended the session ──
    st.files.abort_all().await;
    stop_drive(&ctx, &mut st).await;
    if let Some(cast) = st.cast.take() {
        cast.stop().await;
    }
    if let Some(fp) = &my_fp {
        // Only remove our own entry (a newer session of the same device may have replaced it).
        let mut sessions = ctx.sessions.lock().unwrap();
        if sessions.get(fp).is_some_and(|(_, tx)| tx.same_channel(&st.out_tx)) {
            sessions.remove(fp);
        }
    }
    if let Some(screen) = st.deskflow_screen.take() {
        ctx.deskflow.client_left(&sysinfo::hostname(), &screen, &ctx.layouts.lock().unwrap());
    }
    if let Some(fp) = &my_fp {
        drop(_online);
        if !ctx.online.lock().unwrap().contains_key(fp) {
            ctx.shell.set_device_status(fp, &st.name, st.kind, None, false, false).await;
            if ctx.settings.lock().unwrap().lock_when_gone.contains(fp) {
                tokio::spawn(lock_if_still_gone(ctx.clone(), fp.clone(), st.name.clone()));
            }
        }
    }
    info!(?peer, "disconnected");
    result
}

struct SessionState {
    /// The phone's paired name (for notifications / indicator).
    name: String,
    /// Gamepad mode: this phone's virtual controller.
    gamepad: Option<crate::gamepad::Gamepad>,
    files: crate::files::Receiver,
    /// Messages produced outside the loop (file requests from FUSE threads).
    out_tx: tokio::sync::mpsc::UnboundedSender<WsMessage>,
    drive: Option<(Arc<crate::drive::DriveLink>, crate::drive::Mounted)>,
    /// Set when the peer is a PC that announced a Deskflow client screen.
    deskflow_screen: Option<String>,
    /// "phone" or "pc", from the device's Hello (top-bar icon).
    kind: &'static str,
    /// Screen cast to this phone, while running.
    cast: Option<crate::cast::Cast>,
    /// A cast being set up (the PC's user may still be picking a screen);
    /// picked up by the session loop.
    cast_starting: Option<tokio::sync::oneshot::Receiver<Result<crate::cast::Cast>>>,
    /// False after `CastStop`, so a cast that finishes starting is dropped.
    cast_wanted: bool,
    /// Viewer window showing this phone's screen, while it mirrors.
    mirror: Option<crate::mirror::Mirror>,
    /// The drive's label while it's unmounted for sleep (remounted on wake).
    asleep_label: Option<String>,
    /// The device's certificate fingerprint (None for the local CLI/app).
    fp: Option<String>,
}

/// Next item of an optional receiver; never resolves without one.
pub(crate) async fn recv_opt<T>(rx: &mut Option<tokio::sync::mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

pub(crate) async fn oneshot_opt<T>(rx: &mut Option<tokio::sync::oneshot::Receiver<T>>) -> Result<T, tokio::sync::oneshot::error::RecvError> {
    match rx {
        Some(r) => r.await,
        None => std::future::pending().await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn session_loop<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    peer: &Peer,
    ctx: &Arc<Ctx>,
    my_fp: &Option<String>,
    st: &mut SessionState,
    media_rx: &mut tokio::sync::watch::Receiver<tura_proto::NowPlaying>,
    clip_rx: &mut broadcast::Receiver<String>,
    revoked: &mut broadcast::Receiver<String>,
    out_rx: &mut tokio::sync::mpsc::UnboundedReceiver<WsMessage>,
) -> Result<()> {

    ws.send(frame(Message::Hello { protocol: PROTOCOL_VERSION, device_name: sysinfo::hostname(), device_kind: DeviceKind::Pc }))
        .await?;
    ws.send(frame(pc_info(ctx).await)).await?;
    if my_fp.is_some() {
        let np = media_rx.borrow_and_update().clone();
        ws.send(frame(Message::NowPlaying(np))).await?;
    }

    let mut deadline = tokio::time::Instant::now() + PHONE_IDLE;
    // Screen cast: setup in progress, and the encoded frames while running.
    let mut cast_starting = None;
    let mut cast_frames: Option<tokio::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut cast_seq: u64 = 0;
    let mut sleep_rx = ctx.sleep.subscribe();
    loop {
        if let Some(rx) = st.cast_starting.take() {
            cast_starting = Some(rx);
        }
        let next = tokio::select! {
            Ok(going) = sleep_rx.recv() => {
                if going {
                    if let Some((_, m)) = &st.drive {
                        st.asleep_label = Some(m.label.clone());
                    }
                    stop_drive(ctx, st).await;
                } else if let Some(label) = st.asleep_label.take()
                    && let Err(e) = mount_drive(ctx, st, label).await
                {
                    warn!(?peer, "remounting after sleep: {e:#}");
                }
                continue;
            }
            started = oneshot_opt(&mut cast_starting) => {
                cast_starting = None;
                match started {
                    Ok(Ok(mut cast)) if st.cast_wanted => {
                        cast_frames = cast.frames.take();
                        ws.send(frame(Message::CastStarted { width: cast.width, height: cast.height })).await?;
                        st.cast = Some(cast);
                    }
                    Ok(Ok(cast)) => cast.stop().await,
                    Ok(Err(e)) => {
                        warn!(?peer, "cast: {e:#}");
                        ws.send(frame(Message::CastEnded { reason: Some(format!("{e:#}")) })).await?;
                    }
                    Err(_) => {}
                }
                continue;
            }
            au = recv_opt(&mut cast_frames) => {
                match au {
                    Some(au) => {
                        cast_seq += 1;
                        ws.send(WsMessage::Binary(tura_proto::bin::video(cast_seq, &au).into())).await?;
                    }
                    None => {
                        // The encoder stopped: we stopped it, or sharing was ended on the PC.
                        cast_frames = None;
                        if let Some(cast) = st.cast.take() {
                            cast.stop().await;
                            ws.send(frame(Message::CastEnded { reason: Some("Screen sharing was stopped on the PC".into()) })).await?;
                        }
                    }
                }
                continue;
            }
            _ = tokio::time::sleep_until(deadline), if my_fp.is_some() => {
                bail!("no traffic for {} s; dropping stale connection", PHONE_IDLE.as_secs());
            }
            Some(m) = out_rx.recv() => {
                ws.send(m).await?;
                continue;
            }
            Ok(text) = clip_rx.recv(), if my_fp.is_some() && ctx.clipboard_on() => {
                ws.send(frame(Message::Clipboard { text })).await?;
                continue;
            }
            Ok(()) = media_rx.changed(), if my_fp.is_some() => {
                let np = media_rx.borrow_and_update().clone();
                ws.send(frame(Message::NowPlaying(np))).await?;
                continue;
            }
            f = next_frame(ws) => {
                deadline = tokio::time::Instant::now() + PHONE_IDLE;
                match f {
                    Ok(Incoming::Msg(m)) => Ok(Some(m)),
                    Ok(Incoming::Closed) => Ok(None),
                    Ok(Incoming::Activity) => continue,
                    Ok(Incoming::Binary(b)) => {
                        let split = tura_proto::bin::split(&b);
                        if let (Some(m), Some((tura_proto::bin::VIDEO, _, au))) = (st.mirror.as_mut(), split) {
                            m.feed(au);
                        } else if let Some((link, _)) = &st.drive {
                            link.complete_binary(&b);
                        }
                        continue;
                    }
                    Err(e) => Err(e),
                }
            }
            Ok(fp) = revoked.recv(), if my_fp.is_some() => {
                if Some(&fp) == my_fp.as_ref() {
                    info!(?peer, "unpaired — closing session");
                    let _ = ws.send(frame(Message::Error { message: "not paired: this phone was unpaired on the PC".into() })).await;
                    let _ = ws.close(None).await;
                    break;
                }
                continue;
            }
        };
        let msg = match next {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                ws.send(frame(Message::Error { message: format!("{e:#}") })).await?;
                continue;
            }
        };
        match dispatch(msg, peer, ctx, st).await {
            Ok(Some(reply)) => ws.send(frame(reply)).await?,
            Ok(None) => {}
            Err(e) => {
                warn!(?peer, "{e:#}");
                ws.send(frame(Message::Error { message: power::friendly(&e) })).await?;
            }
        }
    }
    Ok(())
}

async fn pc_info(ctx: &Ctx) -> Message {
    Message::PcInfo {
        hostname: sysinfo::hostname(),
        wol_macs: sysinfo::wol_macs(),
        power_actions: ctx.policy().allowed(),
        desktop: crate::deskflow::graphical_session().await.is_some(),
        bluetooth: crate::bluetooth::address().await,
    }
}

const KEY_ESC: u16 = 1;
const KEY_ENTER: u16 = 28;

/// At GNOME's login screen: wake it, pick the (first) user, type the password.
/// Never logs the password. Ok(true) once a desktop session exists.
async fn log_in(ctx: &Ctx, password: &str) -> Result<bool> {
    let presses = {
        let mut km = ctx.keymap.lock().unwrap_or_else(|p| p.into_inner());
        let km = km.get()?;
        let mut presses = vec![];
        for ch in password.chars() {
            presses.extend(km.strokes(ch).context("the password has a character the PC's keyboard layout can't type")?);
        }
        presses
    };
    let key = |code: u16| -> Result<()> {
        let mut input = ctx.input.lock().unwrap_or_else(|p| p.into_inner());
        input.get()?.key(code, tura_proto::PressState::Click)
    };
    // Esc wakes the screen / lifts the curtain (and leaves a half-typed prompt);
    // Enter picks the highlighted user.
    key(KEY_ESC)?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    key(KEY_ENTER)?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    {
        let mut input = ctx.input.lock().unwrap_or_else(|p| p.into_inner());
        input.get()?.type_presses(&presses, &[])?;
    }
    key(KEY_ENTER)?;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if crate::deskflow::graphical_session().await.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn dispatch(msg: Message, peer: &Peer, ctx: &Arc<Ctx>, st: &mut SessionState) -> Result<Option<Message>> {
    let with_input = |f: &dyn Fn(&mut Input) -> Result<()>| -> Result<Option<Message>> {
        // Short, non-async syscalls: a std mutex is fine here.
        let mut guard = ctx.input.lock().unwrap_or_else(|p| p.into_inner());
        f(guard.get()?)?;
        Ok(None)
    };

    match msg {
        Message::Hello { device_name, protocol, device_kind } => {
            info!(%device_name, protocol, ?device_kind, "peer hello");
            st.kind = if device_kind == DeviceKind::Pc { "pc" } else { "phone" };
            if let Peer::Phone { fingerprint, .. } = peer {
                ctx.pairing.set_kind(fingerprint, st.kind);
            }
            // A joined PC: offer it our Deskflow server (shared mouse/keyboard).
            if device_kind == DeviceKind::Pc && matches!(peer, Peer::Phone { .. }) && crate::deskflow::Deskflow::available() {
                match ctx.deskflow.fingerprint() {
                    Ok(fingerprint) => {
                        let hello = Message::DeskflowHello {
                            role: tura_proto::DeskflowRole::Server,
                            screen: sysinfo::hostname(),
                            fingerprint,
                            port: crate::deskflow::PORT,
                        };
                        let _ = st.out_tx.send(frame(hello));
                    }
                    Err(e) => warn!("deskflow certificate: {e:#}"),
                }
            }
            Ok(None)
        }
        Message::DeskflowHello { role: tura_proto::DeskflowRole::Client, screen, fingerprint, .. } => {
            let Peer::Phone { .. } = peer else { return Ok(None) };
            if screen.is_empty() || screen.len() > 64 || !screen.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
                bail!("invalid screen name");
            }
            if fingerprint.len() != 64 || !fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("invalid deskflow fingerprint");
            }
            info!(%screen, "PC joined shared mouse/keyboard");
            ctx.deskflow.client_joined(&sysinfo::hostname(), &screen, &fingerprint, &ctx.layouts.lock().unwrap());
            st.deskflow_screen = Some(screen);
            Ok(None)
        }
        Message::JoinPc { uri } => {
            local_only(peer, "joining a PC")?;
            let cfg = crate::peer::PeerConfig::from_uri(&uri)?;
            cfg.save()?;
            info!(name = %cfg.name, "joining PC");
            let _ = ctx.peer_cfg.send(Some(cfg));
            Ok(None)
        }
        Message::ForDevice { device, message } => {
            local_only(peer, "sending to a device")?;
            let Some(link) = find_link(ctx, &device) else { bail!("{device:?} isn't connected") };
            link.out.send(frame(*message)).map_err(|_| anyhow::anyhow!("{device:?} just disconnected"))?;
            Ok(None)
        }
        Message::Login { password } => {
            let Peer::Phone { .. } = peer else { bail!("log in from a paired device") };
            if crate::deskflow::graphical_session().await.is_some() {
                return Ok(Some(Message::LoginResult { ok: true, message: Some("Already logged in".into()) }));
            }
            if password.is_empty() || password.len() > 512 {
                bail!("invalid password");
            }
            info!("logging in at the login screen for {}", st.name);
            // In the background: it takes a while, and the session loop must keep answering pings.
            let (ctx, out) = (ctx.clone(), st.out_tx.clone());
            tokio::spawn(async move {
                let reply = match log_in(&ctx, &password).await {
                    Ok(true) => {
                        info!("logged in from the phone");
                        Message::LoginResult { ok: true, message: None }
                    }
                    Ok(false) => Message::LoginResult { ok: false, message: Some("No desktop started: wrong password?".into()) },
                    Err(e) => Message::LoginResult { ok: false, message: Some(format!("{e:#}")) },
                };
                let _ = out.send(frame(reply));
                let _ = out.send(frame(pc_info(&ctx).await));
            });
            Ok(None)
        }
        Message::CastStart { max_width, max_height } => {
            let Peer::Phone { .. } = peer else { bail!("only a phone can receive the screen") };
            if let Some(cast) = st.cast.take() {
                cast.stop().await;
            }
            st.cast_wanted = true;
            let (tx, rx) = tokio::sync::oneshot::channel();
            // Not awaited here: the PC's user may take a while to pick a screen.
            tokio::spawn(async move {
                let _ = tx.send(crate::cast::Cast::start(max_width, max_height).await);
            });
            st.cast_starting = Some(rx);
            Ok(None)
        }
        Message::CastStop => {
            st.cast_wanted = false;
            if let Some(cast) = st.cast.take() {
                cast.stop().await;
            }
            Ok(None)
        }
        Message::MirrorStart { width, height } => {
            let Peer::Phone { .. } = peer else { bail!("only a device can mirror its screen") };
            st.mirror = None; // a restart (e.g. after reconnecting) replaces the window
            st.mirror = Some(crate::mirror::Mirror::start(&st.name, width.clamp(16, 8192), height.clamp(16, 8192), st.out_tx.clone(), false)?);
            Ok(None)
        }
        Message::MirrorStop => {
            if st.mirror.take().is_some() {
                info!("{} stopped showing its screen", st.name);
            }
            Ok(None)
        }
        Message::Gamepad { buttons, lx, ly, rx, ry, lt, rt } => {
            let Peer::Phone { .. } = peer else { bail!("gamepad mode is for phones") };
            if st.gamepad.is_none() {
                st.gamepad = Some(crate::gamepad::Gamepad::new(&st.name)?);
                info!("{} is now a gamepad", st.name);
            }
            if let Some(pad) = &mut st.gamepad {
                pad.update(crate::gamepad::State { buttons, lx, ly, rx, ry, lt, rt })?;
            }
            Ok(None)
        }
        Message::GamepadEnd => {
            if st.gamepad.take().is_some() {
                info!("{} is no longer a gamepad", st.name);
            }
            Ok(None)
        }
        Message::BtPairStart { name } => {
            let Peer::Phone { .. } = peer else { bail!("only a paired phone can pair over Bluetooth") };
            let out = st.out_tx.clone();
            let ready_out = out.clone();
            tokio::spawn(async move {
                let res = crate::bluetooth::pair_window(name, move || {
                    let _ = ready_out.send(frame(Message::BtPairReady));
                })
                .await;
                let _ = out.send(frame(Message::BtPairResult { ok: res.is_ok(), message: res.err().map(|e| format!("{e:#}")) }));
            });
            Ok(None)
        }
        Message::PointerAbs { x, y } => {
            if let Some(cast) = &st.cast {
                cast.pointer(x, y).await?;
            }
            Ok(None)
        }
        Message::ScreenView { device } => {
            local_only(peer, "viewing a screen")?;
            let Some(link) = find_link(ctx, &device) else { bail!("{device:?} isn't connected") };
            let request = if ctx.pairing.kind(&link.key).as_deref() == Some("phone") {
                Message::MirrorRequest
            } else {
                Message::CastStart { max_width: VIEW_MAX.0, max_height: VIEW_MAX.1 }
            };
            link.out.send(frame(request)).map_err(|_| anyhow::anyhow!("{device:?} just disconnected"))?;
            Ok(None)
        }
        // A PC we asked to cast (`ScreenView`): show it.
        Message::CastStarted { width, height } => {
            let Peer::Phone { .. } = peer else { return Ok(None) };
            st.mirror = None;
            st.mirror = Some(crate::mirror::Mirror::start(&st.name, width.clamp(16, 8192), height.clamp(16, 8192), st.out_tx.clone(), true)?);
            Ok(None)
        }
        Message::CastEnded { reason } => {
            if st.mirror.take().is_some()
                && let Some(reason) = reason
            {
                let _ = ctx.notifier.show("Tunnelrat", "video-display-symbolic", &format!("{}'s screen", st.name), &reason, 0).await;
            }
            Ok(None)
        }
        Message::GetSettings => {
            local_only(peer, "reading settings")?;
            Ok(Some(Message::Settings { settings: ctx.settings.lock().unwrap().clone() }))
        }
        Message::SetSettings { settings } => {
            local_only(peer, "changing settings")?;
            crate::settings::save(&settings)?;
            *ctx.settings.lock().unwrap() = settings.clone();
            ctx.deskflow.set_enabled(settings.shared_input);
            ctx.shell.set_hidden(&settings.hidden_chips).await;
            info!(?settings, "settings changed");
            Ok(Some(Message::Settings { settings }))
        }
        Message::GetLayouts => {
            local_only(peer, "reading the layout")?;
            let layouts = ctx.layouts.lock().unwrap();
            let entries = layouts.entries().map(|(device, p)| tura_proto::LayoutEntry { device, side: p.side, from: p.from, to: p.to }).collect();
            Ok(Some(Message::Layouts { entries }))
        }
        Message::GetStatus => {
            local_only(peer, "reading the status")?;
            Ok(Some(Message::Status { items: status(ctx).await }))
        }
        Message::ShareInput => {
            local_only(peer, "turning shared input back on")?;
            ctx.deskflow.share_input();
            Ok(None)
        }
        Message::SetLayout { device, side, from, to } => {
            local_only(peer, "changing the layout")?;
            if from >= to || to > 100 {
                bail!("range must be 0 ≤ from < to ≤ 100");
            }
            let mut layouts = ctx.layouts.lock().unwrap();
            layouts.set(&device, crate::deskflow::Placement { side, from, to });
            layouts.save(&identity::config_dir())?;
            ctx.deskflow.update_server(&sysinfo::hostname(), &layouts);
            Ok(None)
        }
        // While this phone watches our screen, its pointer goes through the
        // cast's remote-desktop session (same device as `PointerAbs`).
        m @ (Message::PointerMove { .. } | Message::Scroll { .. } | Message::PointerButton { .. }) if st.cast.is_some() => {
            if let Some(cast) = &st.cast {
                cast.input(&m).await?;
            }
            Ok(None)
        }
        Message::PointerMove { dx, dy } => with_input(&|i| i.pointer_move(dx, dy)),
        Message::Scroll { dx, dy } => with_input(&|i| i.scroll(dx, dy)),
        Message::PointerButton { button, state } => with_input(&|i| i.button(button, state)),
        Message::Key { code, state, modifiers } => {
            if modifiers.is_empty() {
                with_input(&|i| i.key(code, state))
            } else {
                let held = modifier_keys(&modifiers);
                let press = crate::keymap::Press { key: code, shift: false, altgr: false };
                with_input(&|i| i.type_presses(&[press], &held))
            }
        }
        Message::Text { text, modifiers } => {
            let (presses, missing) = {
                let mut km = ctx.keymap.lock().unwrap_or_else(|p| p.into_inner());
                let km = km.get()?;
                let mut presses = vec![];
                let mut missing = String::new();
                for ch in text.chars() {
                    match km.strokes(ch) {
                        Some(p) => presses.extend(p),
                        None if !missing.contains(ch) => missing.push(ch),
                        None => {}
                    }
                }
                (presses, missing)
            };
            let held = modifier_keys(&modifiers);
            with_input(&|i| i.type_presses(&presses, &held))?;
            if !missing.is_empty() {
                bail!("can't type {missing:?} with the PC's keyboard layout");
            }
            Ok(None)
        }
        Message::Power { action } => {
            info!(?action, "power action requested");
            power::run(action, ctx.policy()).await?;
            Ok(None)
        }
        Message::PairBegin => {
            local_only(peer, "pairing")?;
            let token = ctx.pairing.new_token();
            let uri = pair_uri(ctx, &token);
            let qr = qr_rows(&uri);
            Ok(Some(Message::PairOffer { uri, expires_in_secs: TOKEN_TTL.as_secs(), qr }))
        }
        Message::Media { action } => {
            ctx.media.command(action).await?;
            Ok(None)
        }
        Message::VolumeStep { step } => {
            ctx.media.volume_step(step).await?;
            Ok(None)
        }
        Message::MuteToggle => {
            ctx.media.mute_toggle().await?;
            Ok(None)
        }
        Message::Clipboard { text } => {
            if !ctx.clipboard_on() {
                return Ok(None);
            }
            if text.len() > 256 * 1024 {
                bail!("clipboard text too large");
            }
            // Relay first: even without the GNOME extension here, other devices still sync.
            let changed = match ctx.shell.set_clipboard(&text).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.shell.relay(&text);
                    return Err(e);
                }
            };
            if changed {
                ctx.shell.relay(&text);
            }
            info!(chars = text.chars().count(), changed, "clipboard from {}", st.name);
            Ok(None)
        }
        Message::Battery { level, charging } => {
            let Peer::Phone { fingerprint, .. } = peer else { return Ok(None) };
            let level = level.min(100);
            let prev = ctx.battery.lock().unwrap().insert(fingerprint.clone(), (level, charging));
            ctx.shell.set_device_status(fingerprint, &st.name, st.kind, Some(level), charging, true).await;
            // Warn once when crossing into low battery while unplugged.
            let was_low = prev.is_some_and(|(l, c)| l <= LOW_BATTERY && !c);
            if level <= LOW_BATTERY && !charging && !was_low {
                let _ = ctx
                    .notifier
                    .show("Tunnelrat", "battery-level-10-symbolic", &format!("{} battery low", st.name), &format!("{level}% remaining"), 0)
                    .await;
            }
            Ok(None)
        }
        Message::PhoneNotification { key, app, title, text, can_reply } => {
            let Peer::Phone { fingerprint, .. } = peer else { return Ok(None) };
            let map_key = (fingerprint.clone(), key.clone());
            let replaces = ctx.phone_notifs.lock().unwrap().get(&map_key).copied().unwrap_or(0);
            let summary = if title.is_empty() { app.clone() } else { title };
            let source = format!("{} · {app}", st.name);
            let id = if can_reply {
                ctx.notifier.show_reply(&source, &summary, &text, replaces, fingerprint, &key, &format!("{source}: {summary}")).await?
            } else {
                ctx.notifier.show(&source, "phone-symbolic", &summary, &text, replaces).await?
            };
            let mut map = ctx.phone_notifs.lock().unwrap();
            if map.len() > 500 {
                map.clear(); // bound memory; worst case an old one can't be auto-closed
            }
            map.insert(map_key, id);
            Ok(None)
        }
        Message::PhoneNotificationRemoved { key } => {
            let Peer::Phone { fingerprint, .. } = peer else { return Ok(None) };
            let id = ctx.phone_notifs.lock().unwrap().remove(&(fingerprint.clone(), key));
            if let Some(id) = id {
                ctx.notifier.close(id).await;
            }
            Ok(None)
        }
        Message::OpenUrl { url } => {
            if !crate::files::valid_url(&url) {
                bail!("only http(s) links can be opened");
            }
            info!(%url, "opening link from phone");
            tokio::process::Command::new("gio").arg("open").arg(&url).spawn().context("running gio open")?;
            Ok(None)
        }
        Message::FileStart { id, name, size } => {
            st.files.start(id, &name, size).await?;
            Ok(None)
        }
        Message::FileChunk { id, data } => {
            st.files.chunk(id, &data).await?;
            Ok(None)
        }
        Message::FileEnd { id } => {
            let path = st.files.end(id).await?;
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            info!(path = %path.display(), "file received");
            let _ = ctx
                .notifier
                .show_file("Tunnelrat", &format!("Received from {}", st.name), &file, path.clone())
                .await;
            Ok(Some(Message::FileSaved { id, path: path.display().to_string() }))
        }
        Message::DriveOffer { label } => {
            let Peer::Phone { .. } = peer else { bail!("only phones can share a drive") };
            // The phone re-offers on every reconnect/settings change; keep a
            // matching mount instead of unmounting under open file managers.
            let label = label.map(|l| if l.trim().is_empty() { st.name.clone() } else { l });
            if st.drive.as_ref().map(|(_, m)| &m.label) == label.as_ref() {
                return Ok(None);
            }
            stop_drive(ctx, st).await;
            st.asleep_label = None;
            if let Some(label) = label {
                mount_drive(ctx, st, label).await?;
            }
            Ok(None)
        }
        Message::FsReply { id, result } => {
            if let Some((link, _)) = &st.drive {
                link.complete(id, result);
            }
            Ok(None)
        }
        Message::ListDevices => {
            local_only(peer, "listing devices")?;
            let peer = ctx.peer_cfg.borrow().as_ref().map(|c| DeviceInfo {
                name: c.name.clone(),
                fingerprint: c.fingerprint.clone(),
                paired_at: 0,
                online: u32::from(ctx.peer_out.lock().unwrap().is_some()),
                battery: None,
                charging: false,
                kind: Some(DeviceKind::Pc),
                in_top_bar: false,
                drive: None,
            });
            Ok(Some(Message::Devices { devices: ctx.pairing.list().iter().map(|d| ctx.device_info(d)).collect(), peer }))
        }
        Message::Unpair { selector } => {
            local_only(peer, "unpairing")?;
            let removed = ctx.pairing.remove(&selector)?;
            let device = ctx.device_info(&removed);
            info!(name = %removed.name, fingerprint = %removed.fingerprint, "device unpaired from the PC");
            let _ = ctx.revoked.send(removed.fingerprint);
            Ok(Some(Message::Unpaired { device }))
        }
        Message::ForgetMe => {
            let Peer::Phone { fingerprint, .. } = peer else { bail!("only phones can send forget_me") };
            if let Some(d) = ctx.pairing.remove_fingerprint(fingerprint)? {
                info!(name = %d.name, "phone unpaired itself");
            }
            let _ = ctx.revoked.send(fingerprint.clone());
            Ok(None)
        }
        Message::Ping => Ok(Some(Message::Pong)),
        Message::Pair { .. } => Ok(None), // already paired; ignore
        Message::Pong
        | Message::PcInfo { .. }
        | Message::BtPairReady
        | Message::Ring
        | Message::BtPairResult { .. }
        | Message::Error { .. }
        | Message::PairOk { .. }
        | Message::PairOffer { .. }
        | Message::Devices { .. }
        | Message::Unpaired { .. }
        | Message::NowPlaying(_)
        | Message::FileSaved { .. }
        | Message::FsRequest { .. }
        | Message::DeskflowHello { .. }
        | Message::PhoneGesture { .. }
        | Message::PhoneTouch { .. }
        | Message::MirrorRequest
        | Message::PhoneReply { .. }
        | Message::LoginResult { .. }
        | Message::Settings { .. }
        | Message::Layouts { .. }
        | Message::Status { .. }
        | Message::PhoneNav { .. }
        | Message::PhoneText { .. } => Ok(None),
    }
}

/// A connection to another device: its fingerprint, name and outgoing queue.
struct Link {
    key: String,
    out: tokio::sync::mpsc::UnboundedSender<WsMessage>,
}

/// A connected device by name (case-insensitive) or fingerprint prefix: a
/// paired phone/PC connected to us, or the PC we joined.
fn find_link(ctx: &Ctx, device: &str) -> Option<Link> {
    let sel = device.to_lowercase();
    let found = ctx
        .sessions
        .lock()
        .unwrap()
        .iter()
        .find(|(fp, (name, _))| name.to_lowercase() == sel || (sel.len() >= 4 && fp.starts_with(&sel)))
        .map(|(fp, (_, tx))| Link { key: fp.clone(), out: tx.clone() });
    if found.is_some() {
        return found;
    }
    let (_, out) = ctx.peer_out.lock().unwrap().clone().filter(|(n, _)| n.to_lowercase() == sel)?;
    let key = ctx.peer_cfg.borrow().as_ref().map(|c| c.fingerprint.clone())?;
    Some(Link { key, out })
}

/// Mounts the session's phone under `label`.
async fn mount_drive(ctx: &Ctx, st: &mut SessionState, label: String) -> Result<()> {
    let link = crate::drive::DriveLink::new(st.out_tx.clone());
    let reg = ctx.clone_mounts_handle();
    let l2 = link.clone();
    let mounted = tokio::task::spawn_blocking(move || crate::drive::mount(&label, l2, &reg)).await??;
    info!(dir = %mounted.dir.display(), "phone mounted");
    if let Some(fp) = &st.fp {
        ctx.drives.lock().unwrap().insert(fp.clone(), mounted.dir.display().to_string());
    }
    st.drive = Some((link, mounted));
    Ok(())
}

/// Unmounts the session's drive, if any, and fails its pending calls.
async fn stop_drive(ctx: &Ctx, st: &mut SessionState) {
    if let Some(fp) = &st.fp {
        ctx.drives.lock().unwrap().remove(fp);
    }
    if let Some((link, mounted)) = st.drive.take() {
        link.fail_all();
        let reg = ctx.clone_mounts_handle();
        let dir = mounted.dir.clone();
        let _ = tokio::task::spawn_blocking(move || crate::drive::unmount(mounted, &reg)).await;
        info!(dir = %dir.display(), "phone unmounted");
    }
}

const LOW_BATTERY: u8 = 15;

/// evdev keys for shortcut modifiers (left-hand variants).
fn modifier_keys(mods: &[Modifier]) -> Vec<u16> {
    mods.iter()
        .map(|m| match m {
            Modifier::Ctrl => 29,   // KEY_LEFTCTRL
            Modifier::Alt => 56,    // KEY_LEFTALT
            Modifier::Shift => 42,  // KEY_LEFTSHIFT
            Modifier::Super => 125, // KEY_LEFTMETA
        })
        .collect()
}

fn local_only(peer: &Peer, what: &str) -> Result<()> {
    if !matches!(peer, Peer::Local) {
        bail!("{what} can only be done on the PC itself");
    }
    Ok(())
}

/// The QR code of `text`: one string of '1' (dark) / '0' per row.
fn qr_rows(text: &str) -> Vec<String> {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else { return vec![] };
    let w = code.width();
    let colors = code.to_colors();
    colors.chunks(w).map(|row| row.iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect()).collect()
}

/// The status page's checklist.
async fn status(ctx: &Ctx) -> Vec<tura_proto::StatusItem> {
    use tura_proto::StatusItem;
    let item = |name: &str, ok: bool, detail: String| StatusItem { name: name.into(), ok, detail };
    let mut items = vec![];

    items.push(item(
        "Desktop clipboard",
        ctx.shell.active(),
        if ctx.shell.active() { "Clipboard bridge available in this desktop session".into() } else { "No supported clipboard tool: install wl-clipboard (Wayland) or xclip (X11)".into() },
    ));
    let input = ctx.input.lock().unwrap_or_else(|p| p.into_inner()).get().is_ok();
    items.push(item("Mouse & keyboard from devices", input, if input { "Can type and move the pointer".into() } else { "No access to /dev/uinput (see the uinput rule in the README)".into() }));
    let (login_ok, login_detail) = uinput_group();
    items.push(item("Typing at the login screen", login_ok, login_detail));
    let linger = linger().await;
    items.push(item(
        "Runs from boot",
        linger,
        if linger { "Reachable before you log in (e.g. after Wake on LAN)".into() } else { "Only while you're logged in: loginctl enable-linger".into() },
    ));
    let mdns = crate::mdns::ANNOUNCED.load(std::sync::atomic::Ordering::Relaxed);
    items.push(item("Found on the network", mdns, if mdns { "Announced via Avahi".into() } else { "Not announced: is avahi-daemon running?".into() }));
    let (df_ok, df) = ctx.deskflow.state();
    items.push(item("Shared mouse/keyboard", df_ok, df));
    let enc = crate::cast::has_element("pipewiresrc").await && (crate::cast::has_element("vah264enc").await || crate::cast::has_element("x264enc").await);
    items.push(item("Screen to phones", enc, if enc { "GStreamer with an H.264 encoder".into() } else { "Needs gstreamer1.0-pipewire and x264 or VA-API".into() }));
    let viewer = crate::mirror::viewer_path().exists() || crate::settings::which("tura-viewer").is_some();
    items.push(item(
        "Screen viewer",
        viewer,
        if viewer { "tura-viewer installed: shows phones' and PCs' screens".into() } else { "Copy pc/viewer/tura-viewer next to turad".into() },
    ));
    let (wol_ok, wol) = wake_on_lan();
    items.push(item("Wake on LAN", wol_ok, wol));
    let drives: Vec<String> = ctx.drives.lock().unwrap().values().cloned().collect();
    items.push(item("Phone drives", true, if drives.is_empty() { "None mounted".into() } else { drives.join(", ") }));
    items
}

/// Whether a wired card may wake the PC (the kernel's per-device wakeup
/// flag, which r8169 & co. set when Wake-on-LAN is on).
fn wake_on_lan() -> (bool, String) {
    let macs = sysinfo::wol_macs();
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else { return (false, "Can't read network interfaces".into()) };
    let mut seen = vec![];
    for e in dir.flatten() {
        let iface = e.file_name().to_string_lossy().into_owned();
        let mac = std::fs::read_to_string(e.path().join("address")).unwrap_or_default();
        if !macs.iter().any(|m| m.eq_ignore_ascii_case(mac.trim())) {
            continue;
        }
        let wake = std::fs::read_to_string(e.path().join("device/power/wakeup")).unwrap_or_default();
        if wake.trim() == "enabled" {
            return (true, format!("{iface} can wake this PC"));
        }
        seen.push(iface);
    }
    match seen.first() {
        None => (false, "No wired network card: Wake on LAN needs Ethernet".into()),
        Some(iface) => (
            false,
            format!("{iface} isn't allowed to wake the PC: sudo nmcli connection modify <wired connection> 802-3-ethernet.wake-on-lan magic, then sudo ethtool -s {iface} wol g"),
        ),
    }
}

/// Whether /dev/uinput belongs to the uinput group and we're in it.
fn uinput_group() -> (bool, String) {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata("/dev/uinput") else { return (false, "/dev/uinput is missing".into()) };
    let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let name = groups.lines().find_map(|l| {
        let mut f = l.split(':');
        let n = f.next()?;
        f.next();
        (f.next()?.parse::<u32>().ok()? == meta.gid()).then(|| n.to_owned())
    });
    if name.as_deref() != Some("uinput") {
        return (false, "Install the uinput udev rule (README: Log in from the phone)".into());
    }
    // SAFETY: getgroups with a correctly sized buffer.
    let mine: Vec<libc::gid_t> = unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        let mut v = vec![0; n.max(0) as usize];
        let n = libc::getgroups(n, v.as_mut_ptr());
        v.truncate(n.max(0) as usize);
        v
    };
    if mine.contains(&meta.gid()) {
        (true, "Ready".into())
    } else {
        (false, "Add yourself to the uinput group and log in again".into())
    }
}

async fn linger() -> bool {
    let Ok(conn) = zbus::Connection::system().await else { return false };
    let Ok(user) = zbus::Proxy::new(&conn, "org.freedesktop.login1", "/org/freedesktop/login1/user/self", "org.freedesktop.login1.User").await else {
        return false;
    };
    user.get_property::<bool>("Linger").await.unwrap_or(false)
}

/// `tunnelrat://pair?v=1&n=<host>&h=<ip,ip>&p=<port>&fp=<sha256>&t=<token>`
fn pair_uri(ctx: &Ctx, token: &str) -> String {
    let hosts: Vec<String> = candidate_addrs().iter().map(ToString::to_string).collect();
    format!(
        "tunnelrat://pair?v=1&n={}&h={}&p={}&fp={}&t={}",
        sysinfo::hostname(),
        hosts.join(","),
        ctx.port,
        ctx.pc_fingerprint,
        token
    )
}

/// IPv4 addresses a phone could reach: LAN first, then Tailscale. Skips
/// loopback, Docker/libvirt bridges and VPN tunnels like Mullvad.
fn candidate_addrs() -> Vec<IpAddr> {
    let mut lan = vec![];
    let mut tailnet = vec![];
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        let IpAddr::V4(v4) = iface.ip() else { continue };
        let n = iface.name.as_str();
        if v4.is_loopback() || ["docker", "br-", "veth", "virbr", "wg", "lxd", "vboxnet"].iter().any(|p| n.starts_with(p)) {
            continue;
        }
        if n.starts_with("tailscale") {
            tailnet.push(IpAddr::V4(v4));
        } else if v4.is_private() {
            lan.push(IpAddr::V4(v4));
        }
    }
    lan.extend(tailnet);
    lan
}
