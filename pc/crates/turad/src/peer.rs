//! PC ↔ PC: this turad joins another PC's turad, the way the phone does —
//! mutual TLS with the other PC's certificate pinned from its pairing code,
//! then clipboard, battery, power and Deskflow setup over that link.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{info, warn};
use tura_proto::{DeskflowRole, DeviceKind, Message, PROTOCOL_VERSION};

use crate::server::Ctx;
use crate::{identity, power, sysinfo};

/// The PC this one has joined (`tura join`), saved in `peer.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerConfig {
    pub name: String,
    pub hosts: Vec<String>,
    pub port: u16,
    pub fingerprint: String,
    /// One-time pairing token, until the other side confirmed pairing.
    pub token: Option<String>,
}

impl PeerConfig {
    pub fn path() -> std::path::PathBuf {
        identity::config_dir().join("peer.json")
    }

    pub fn load() -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(Self::path()).ok()?).ok()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::write(Self::path(), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Parses `tunnelrat://pair?v=1&n=..&h=a,b&p=..&fp=..&t=..` (from `tura pair`).
    pub fn from_uri(uri: &str) -> Result<Self> {
        let q = uri.trim().strip_prefix("tunnelrat://pair?").context("not a tunnelrat://pair link")?;
        let get = |k: &str| q.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(str::to_owned);
        if get("v").as_deref() != Some("1") {
            bail!("unsupported pairing code version");
        }
        let fingerprint = get("fp").filter(|f| f.len() == 64 && f.chars().all(|c| c.is_ascii_hexdigit())).context("bad fingerprint")?;
        let token = get("t").filter(|t| t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit())).context("bad token")?;
        let hosts: Vec<String> = get("h").context("no addresses")?.split(',').map(str::to_owned).filter(|h| h.parse::<std::net::IpAddr>().is_ok()).collect();
        if hosts.is_empty() {
            bail!("no usable addresses");
        }
        let port = get("p").and_then(|p| p.parse().ok()).context("bad port")?;
        Ok(Self { name: get("n").unwrap_or_else(|| "PC".into()), hosts, port, fingerprint: fingerprint.to_lowercase(), token: Some(token) })
    }
}

// ── TLS: pin the other PC's certificate ──

#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if identity::fingerprint(end) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("PC certificate doesn't match the paired one".into()))
        }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.algs)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.algs)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn tls_config(ctx: &Ctx, pin: &str) -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(Pinned { fingerprint: pin.to_owned(), algs: provider.signature_verification_algorithms });
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![ctx.identity.cert.clone()], ctx.identity.key.clone_key())?;
    Ok(Arc::new(cfg))
}

// ── battery (laptops) ──

/// (percent, charging) from UPower's display device, if this PC has a battery.
async fn battery() -> Option<(u8, bool)> {
    let conn = zbus::Connection::system().await.ok()?;
    let p = zbus::Proxy::new(&conn, "org.freedesktop.UPower", "/org/freedesktop/UPower/devices/DisplayDevice", "org.freedesktop.UPower.Device")
        .await
        .ok()?;
    if !p.get_property::<bool>("IsPresent").await.ok()? {
        return None;
    }
    let pct: f64 = p.get_property("Percentage").await.ok()?;
    let state: u32 = p.get_property("State").await.ok()?; // 1 charging, 4 fully charged
    Some((pct.round().clamp(0.0, 100.0) as u8, state == 1 || state == 4))
}

// ── the link ──

/// Runs forever: keeps a link to the joined PC (if any), reconnecting.
pub async fn run(ctx: Arc<Ctx>) {
    let mut cfg_rx = ctx.peer_cfg.subscribe();
    let mut backoff = Duration::from_secs(1);
    loop {
        let Some(cfg) = cfg_rx.borrow_and_update().clone() else {
            let _ = cfg_rx.changed().await;
            continue;
        };
        let mut linked = false;
        for host in &cfg.hosts {
            let res = tokio::select! {
                r = link(&ctx, &cfg, host) => r,
                _ = cfg_rx.changed() => Ok(()),
            };
            ctx.deskflow.disconnect_from_server();
            linked |= ctx.peer_out.lock().unwrap().take().is_some();
            match res {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(e) => warn!(host, "link to {}: {e:#}", cfg.name),
            }
            if cfg_rx.has_changed().unwrap_or(false) {
                break;
            }
        }
        // None of its addresses answered: maybe it got a new one. Ask the
        // local network (mDNS) and try that first next time.
        if !linked && !cfg_rx.has_changed().unwrap_or(false) {
            rediscover(&ctx, &cfg).await;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

async fn rediscover(ctx: &Ctx, cfg: &PeerConfig) {
    let Some(address) = crate::mdns::find(&cfg.name, &cfg.fingerprint).await else { return };
    if cfg.hosts.first() == Some(&address) {
        return;
    }
    info!(%address, "found {} on the local network", cfg.name);
    let mut c = cfg.clone();
    c.hosts.retain(|h| h != &address);
    c.hosts.insert(0, address);
    c.hosts.truncate(6);
    if let Err(e) = c.save() {
        warn!("saving peer.json: {e:#}");
    }
    // Silently: the run loop picks it up on its next round.
    ctx.peer_cfg.send_if_modified(|v| {
        *v = Some(c);
        false
    });
}

async fn link(ctx: &Arc<Ctx>, cfg: &PeerConfig, host: &str) -> Result<()> {
    let tcp = tokio::time::timeout(Duration::from_secs(4), TcpStream::connect((host, cfg.port))).await.context("timed out")??;
    tcp.set_nodelay(true)?;
    let connector = tokio_rustls::TlsConnector::from(tls_config(ctx, &cfg.fingerprint)?);
    let tls = connector.connect(ServerName::try_from("tunnelrat.local")?.to_owned(), tcp).await.context("TLS")?;
    let (ws, _) = tokio_tungstenite::client_async("ws://tunnelrat.local/", tls).await?;
    let (mut tx, mut rx) = ws.split();
    let text = |m: Message| WsMessage::Text(m.to_json().into());
    let me = sysinfo::hostname();

    if let Some(token) = &cfg.token {
        tx.send(text(Message::Pair { token: token.clone(), device_name: me.clone() })).await?;
    }
    tx.send(text(Message::Hello { protocol: PROTOCOL_VERSION, device_name: me.clone(), device_kind: DeviceKind::Pc })).await?;
    if crate::deskflow::Deskflow::available() {
        match ctx.deskflow.fingerprint() {
            Ok(fp) => tx.send(text(Message::DeskflowHello { role: DeskflowRole::Client, screen: me.clone(), fingerprint: fp, port: 0 })).await?,
            Err(e) => warn!("deskflow certificate: {e:#}"),
        }
    }

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
    *ctx.peer_out.lock().unwrap() = Some((cfg.name.clone(), out_tx.clone()));
    // Our screen to the other PC (it asked), and its screen here (we asked).
    let mut cast: Option<crate::cast::Cast> = None;
    let mut cast_starting: Option<tokio::sync::oneshot::Receiver<Result<crate::cast::Cast>>> = None;
    let mut cast_frames: Option<tokio::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut cast_wanted = false;
    let mut cast_seq: u64 = 0;
    let mut viewer: Option<crate::mirror::Mirror> = None;
    let mut clip_rx = ctx.shell.clipboard_changes();
    let mut ping = tokio::time::interval(Duration::from_secs(15));
    let mut batt_tick = tokio::time::interval(Duration::from_secs(30));
    let mut last_batt = None;
    info!(host, "linked to {}", cfg.name);

    loop {
        tokio::select! {
            frame = rx.next() => {
                let Some(frame) = frame else { bail!("connection closed") };
                let t = match frame? {
                    WsMessage::Text(t) => t,
                    WsMessage::Binary(b) => {
                        if let (Some(v), Some((tura_proto::bin::VIDEO, _, au))) = (viewer.as_mut(), tura_proto::bin::split(&b)) {
                            v.feed(au);
                        }
                        continue;
                    }
                    WsMessage::Close(_) => bail!("connection closed"),
                    _ => continue,
                };
                let Ok(m) = Message::from_json(&t) else { continue };
                match m {
                    Message::PairOk { pc_name } => {
                        info!("paired with {pc_name}");
                        let mut c = cfg.clone();
                        c.token = None;
                        c.save()?;
                        // Update silently: notifying would make run() tear down this link.
                        ctx.peer_cfg.send_if_modified(|v| { *v = Some(c); false });
                    }
                    Message::Error { message } => {
                        warn!("{}: {message}", cfg.name);
                        if message.starts_with("not paired") || message.starts_with("pairing code") {
                            bail!("{message} — run `tura join` with a fresh code");
                        }
                    }
                    Message::DeskflowHello { role: DeskflowRole::Server, fingerprint, port, .. } => {
                        ctx.deskflow.connect_to_server(&me, host, port, &fingerprint);
                    }
                    Message::Clipboard { text } => if ctx.clipboard_on() { let _ = ctx.shell.set_clipboard(&text).await; },
                    // The other PC wants to see our screen.
                    Message::CastStart { max_width, max_height } => {
                        if let Some(c) = cast.take() {
                            c.stop().await;
                        }
                        cast_wanted = true;
                        let (otx, orx) = tokio::sync::oneshot::channel();
                        tokio::spawn(async move { let _ = otx.send(crate::cast::Cast::start(max_width, max_height).await); });
                        cast_starting = Some(orx);
                        info!("{} asked to see this screen", cfg.name);
                    }
                    Message::CastStop => {
                        cast_wanted = false;
                        if let Some(c) = cast.take() {
                            c.stop().await;
                        }
                    }
                    Message::PointerAbs { x, y } => if let Some(c) = &cast { let _ = c.pointer(x, y).await; },
                    m @ (Message::PointerButton { .. } | Message::Scroll { .. } | Message::Key { .. }) => {
                        if cast.is_some() && let Err(e) = ctx.inject(&m) {
                            warn!("input from {}: {e:#}", cfg.name);
                        }
                    }
                    // We asked to see its screen.
                    Message::CastStarted { width, height } => {
                        viewer = None;
                        match crate::mirror::Mirror::start(&cfg.name, width.clamp(16, 8192), height.clamp(16, 8192), out_tx.clone(), true) {
                            Ok(v) => viewer = Some(v),
                            Err(e) => warn!("viewer: {e:#}"),
                        }
                    }
                    Message::CastEnded { reason } => {
                        if viewer.take().is_some() && let Some(reason) = reason {
                            let _ = ctx.notifier.show("Tunnelrat", "video-display-symbolic", &format!("{}'s screen", cfg.name), &reason, 0).await;
                        }
                    }
                    Message::Power { action } => {
                        info!(?action, "power action from {}", cfg.name);
                        if let Err(e) = power::run(action, ctx.policy()).await {
                            tx.send(text(Message::Error { message: power::friendly(&e) })).await?;
                        }
                    }
                    _ => {}
                }
            }
            Some(m) = out_rx.recv() => tx.send(m).await?,
            started = crate::server::oneshot_opt(&mut cast_starting) => {
                cast_starting = None;
                match started {
                    Ok(Ok(mut c)) if cast_wanted => {
                        cast_frames = c.frames.take();
                        tx.send(text(Message::CastStarted { width: c.width, height: c.height })).await?;
                        cast = Some(c);
                    }
                    Ok(Ok(c)) => c.stop().await,
                    Ok(Err(e)) => {
                        warn!("cast: {e:#}");
                        tx.send(text(Message::CastEnded { reason: Some(format!("{e:#}")) })).await?;
                    }
                    Err(_) => {}
                }
            }
            au = crate::server::recv_opt(&mut cast_frames) => match au {
                Some(au) => {
                    cast_seq += 1;
                    tx.send(WsMessage::Binary(tura_proto::bin::video(cast_seq, &au).into())).await?;
                }
                None => {
                    cast_frames = None;
                    if let Some(c) = cast.take() {
                        c.stop().await;
                        tx.send(text(Message::CastEnded { reason: Some(format!("Screen sharing was stopped on {me}")) })).await?;
                    }
                }
            },
            Ok(clip) = clip_rx.recv() => if ctx.clipboard_on() { tx.send(text(Message::Clipboard { text: clip })).await? },
            _ = ping.tick() => tx.send(WsMessage::Ping(Vec::new().into())).await?,
            _ = batt_tick.tick() => {
                let b = battery().await;
                if let Some((level, charging)) = b && b != last_batt {
                    tx.send(text(Message::Battery { level, charging })).await?;
                    last_batt = b;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pair_uri() {
        let fp = "a".repeat(64);
        let t = "b".repeat(32);
        let c = PeerConfig::from_uri(&format!("tunnelrat://pair?v=1&n=Desk&h=192.168.1.20,100.1.2.3,bogus&p=47470&fp={fp}&t={t}")).unwrap();
        assert_eq!(c.name, "Desk");
        assert_eq!(c.hosts, vec!["192.168.1.20", "100.1.2.3"]);
        assert_eq!(c.port, 47470);
        assert_eq!(c.token.as_deref(), Some(t.as_str()));
        assert!(PeerConfig::from_uri("https://evil").is_err());
        assert!(PeerConfig::from_uri(&format!("tunnelrat://pair?v=1&h=1.2.3.4&p=1&fp=xyz&t={t}")).is_err());
    }
}
