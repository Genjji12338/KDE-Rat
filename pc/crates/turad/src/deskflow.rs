//! Shared mouse/keyboard between PCs by driving Deskflow as a separate program.
//!
//! Deskflow is GPL-2.0-only, so it is never linked or copied into Tunnelrat
//! (GPL-3.0): turad only writes its config files and runs `deskflow-core`.
//! Trust comes from Tunnelrat pairing: the PCs exchange Deskflow certificate
//! fingerprints over their already-authenticated link.
//!
//! Files live in `~/.config/tunnelrat/deskflow/{server,client}/` so Deskflow's
//! own GUI settings in `~/.config/Deskflow` stay untouched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rustls_pki_types::{CertificateDer, pem::PemObject};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tura_proto::Side;

pub const PORT: u16 = 24800;

/// Where a peer PC sits relative to this one: `side` of this screen, over
/// the `from..to` percent of that edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub side: Side,
    pub from: u8,
    pub to: u8,
}

impl Default for Placement {
    fn default() -> Self {
        Self { side: Side::Down, from: 0, to: 100 }
    }
}

/// What the supervisor should be running.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Want {
    Nothing,
    /// We're the server; these client screens are connected.
    Server { me: String, clients: BTreeMap<String, Placement>, trusted: Vec<String> },
    /// We're a client of `host:port`.
    Client { me: String, host: String, port: u16, trusted_server: String },
}

pub struct Deskflow {
    root: PathBuf,
    want: watch::Sender<Want>,
    /// Forces a respawn (e.g. after a logout/login: the old process is bound
    /// to the dead desktop session's portal and injects into nothing).
    kick: tokio::sync::Notify,
    /// Peers announced as clients (screen → fingerprint), for the server role.
    /// Kept when a client's link drops: an unchanged config means Deskflow
    /// keeps running, and GNOME doesn't ask for input capture again.
    clients: Mutex<BTreeMap<String, String>>,
    /// The user declined GNOME's input-capture dialog: don't ask again until
    /// re-armed (`tura share-input`) or a new login.
    declined: std::sync::Arc<std::sync::atomic::AtomicBool>,
    rearm: tokio::sync::Notify,
    notifier: std::sync::Arc<crate::notify::Notifier>,
    /// Shared mouse/keyboard switched on in the settings.
    enabled: watch::Sender<bool>,
    /// deskflow-core is running but couldn't get GNOME's remote-desktop
    /// session (e.g. the screen was locked): it doesn't exit, so we restart it.
    retry: std::sync::Arc<tokio::sync::Notify>,
    /// deskflow-core is running right now (status page).
    running: std::sync::atomic::AtomicBool,
}

impl Deskflow {
    pub fn new(config_dir: &Path, notifier: std::sync::Arc<crate::notify::Notifier>) -> Self {
        Self {
            root: config_dir.join("deskflow"),
            want: watch::channel(Want::Nothing).0,
            kick: tokio::sync::Notify::new(),
            clients: Mutex::new(BTreeMap::new()),
            declined: Default::default(),
            rearm: tokio::sync::Notify::new(),
            notifier,
            enabled: watch::channel(true).0,
            retry: Default::default(),
            running: Default::default(),
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.send_if_modified(|e| std::mem::replace(e, on) != on);
    }

    /// One line for the status page: (works, description).
    pub fn state(&self) -> (bool, String) {
        use std::sync::atomic::Ordering::SeqCst;
        if !Self::available() {
            return (false, "Deskflow core is missing; install the current Debian build from github.com/deskflow/deskflow/releases".into());
        }
        if !*self.enabled.borrow() {
            return (true, "Off in the settings".into());
        }
        if self.declined.load(SeqCst) {
            return (false, "GNOME's input-capture request was declined; run `tura share-input`".into());
        }
        if self.running.load(SeqCst) {
            return (true, "Running".into());
        }
        (true, "Waiting for a joined PC".into())
    }

    /// After the input-capture dialog was declined: ask once more.
    pub fn share_input(&self) {
        self.declined.store(false, std::sync::atomic::Ordering::SeqCst);
        self.rearm.notify_one();
    }

    pub fn available() -> bool {
        std::process::Command::new("deskflow-core").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
    }

    fn cert_path(&self) -> PathBuf {
        self.root.join("deskflow.pem")
    }

    /// Our Deskflow certificate fingerprint (`v2:sha256:<hex>` payload),
    /// creating the certificate the way Deskflow's GUI does if needed.
    pub fn fingerprint(&self) -> Result<String> {
        let pem = self.cert_path();
        if !pem.exists() {
            std::fs::create_dir_all(&self.root)?;
            let ok = std::process::Command::new("openssl")
                .args(["req", "-x509", "-nodes", "-days", "3650", "-subj", "/CN=Deskflow", "-newkey", "rsa:2048", "-keyout"])
                .arg(&pem)
                .arg("-out")
                .arg(&pem)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .context("running openssl to create the Deskflow certificate")?
                .success();
            if !ok {
                bail!("openssl failed to create the Deskflow certificate");
            }
            set_private(&pem);
        }
        let cert = CertificateDer::from_pem_file(&pem).context("reading the Deskflow certificate")?;
        Ok(hex::encode(Sha256::digest(cert.as_ref())))
    }

    // ── server side ──

    /// A joined PC announced its Deskflow fingerprint.
    pub fn client_joined(&self, me: &str, screen: &str, fingerprint: &str, layout: &Layouts) {
        self.clients.lock().unwrap().insert(screen.to_owned(), fingerprint.to_owned());
        self.update_server(me, layout);
    }

    /// A joined PC's link dropped. It stays in the server config (Deskflow
    /// just waits for it): restarting Deskflow would make GNOME ask for
    /// input capture again every time a flaky Wi-Fi link reconnects.
    pub fn client_left(&self, _me: &str, screen: &str, _layout: &Layouts) {
        tracing::info!(%screen, "PC left shared mouse/keyboard (kept in the layout)");
    }

    /// Re-applies the layout (after `tura layout`).
    pub fn update_server(&self, me: &str, layout: &Layouts) {
        let clients = self.clients.lock().unwrap().clone();
        let want = if clients.is_empty() {
            Want::Nothing
        } else {
            Want::Server {
                me: me.to_owned(),
                clients: clients.keys().map(|s| (s.clone(), layout.get(s))).collect(),
                trusted: clients.values().cloned().collect(),
            }
        };
        self.want.send_if_modified(|w| std::mem::replace(w, want.clone()) != want);
    }

    // ── client side ──

    pub fn connect_to_server(&self, me: &str, host: &str, port: u16, server_fingerprint: &str) {
        let want = Want::Client { me: me.to_owned(), host: host.to_owned(), port, trusted_server: server_fingerprint.to_owned() };
        self.want.send_if_modified(|w| std::mem::replace(w, want.clone()) != want);
    }

    pub fn disconnect_from_server(&self) {
        self.want.send_if_modified(|w| std::mem::replace(w, Want::Nothing) != Want::Nothing);
    }

    // ── supervisor ──

    /// Restart deskflow-core (if running) so it attaches to the current session.
    pub fn restart(&self) {
        self.kick.notify_one();
    }

    /// Runs forever: restarts Deskflow when the user's graphical session
    /// changes (logout/login), since turad itself outlives the session.
    pub async fn watch_session(&self) {
        let mut last = graphical_session().await;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let now = graphical_session().await;
            if now.is_some() && now != last && last.is_some() {
                tracing::info!("new desktop session; restarting Deskflow");
                self.restart();
            }
            if now.is_some() {
                last = now;
            }
        }
    }

    /// Runs forever: keeps `deskflow-core` matching the wanted state,
    /// restarting it if it exits.
    pub async fn run(&self) {
        let mut rx = self.want.subscribe();
        let mut en_rx = self.enabled.subscribe();
        loop {
            let on = *en_rx.borrow_and_update();
            let want = if on { rx.borrow_and_update().clone() } else { Want::Nothing };
            self.running.store(false, std::sync::atomic::Ordering::SeqCst);
            let child = match &want {
                Want::Nothing => None,
                w => match self.prepare(w).and_then(|(mode, conf)| spawn(mode, &conf, self.declined.clone(), self.retry.clone())) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        tracing::warn!("deskflow: {e:#}");
                        None
                    }
                },
            };
            let Some(mut child) = child else {
                if matches!(want, Want::Nothing) {
                    tokio::select! { _ = rx.changed() => {}, _ = en_rx.changed() => {} }
                } else {
                    tokio::select! { _ = rx.changed() => {}, _ = en_rx.changed() => {}, _ = tokio::time::sleep(Duration::from_secs(10)) => {} }
                }
                continue;
            };
            self.running.store(true, std::sync::atomic::Ordering::SeqCst);
            tokio::select! {
                _ = rx.changed() => {
                    let _ = child.kill().await;
                }
                _ = en_rx.changed() => {
                    let _ = child.kill().await;
                }
                _ = self.retry.notified() => {
                    // Usually the screen was locked; try again until GNOME allows it.
                    let _ = child.kill().await;
                    tracing::info!("deskflow couldn't start its remote-desktop session (screen locked?); retrying in 10 s");
                    tokio::select! { _ = tokio::time::sleep(Duration::from_secs(10)) => {}, _ = self.kick.notified() => {} }
                }
                _ = self.kick.notified() => {
                    let _ = child.kill().await;
                    tokio::time::sleep(Duration::from_secs(2)).await; // let the new session settle
                }
                status = child.wait() => {
                    if self.declined.load(std::sync::atomic::Ordering::SeqCst) {
                        // Restarting would just pop the dialog up again at once.
                        tracing::warn!("input capture declined; shared mouse/keyboard stays off until `tura share-input` or the next login");
                        let _ = self
                            .notifier
                            .show(
                                "Tunnelrat",
                                "input-mouse-symbolic",
                                "Shared mouse/keyboard is off",
                                "Input capture was declined. Run `tura share-input` to be asked again.",
                                0,
                            )
                            .await;
                        tokio::select! {
                            _ = self.rearm.notified() => {}
                            _ = self.kick.notified() => {}
                        }
                        self.declined.store(false, std::sync::atomic::Ordering::SeqCst);
                    } else {
                        tracing::warn!("deskflow-core exited ({status:?}); restarting in 3 s");
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        }
    }

    /// Writes settings + trust (+ server layout) for `want`; returns (mode, settings path).
    fn prepare(&self, want: &Want) -> Result<(&'static str, PathBuf)> {
        let _ = self.fingerprint()?; // ensure the certificate exists
        let cert = self.cert_path();
        // Deskflow log level: 4 = INFO … 7 = DEBUG2 (override with TURAD_DESKFLOW_LOG).
        let log = std::env::var("TURAD_DESKFLOW_LOG").unwrap_or_else(|_| "4".into());
        match want {
            Want::Nothing => bail!("nothing to run"),
            Want::Server { me, clients, trusted } => {
                let dir = self.root.join("server");
                std::fs::create_dir_all(dir.join("tls"))?;
                std::fs::write(dir.join("tls/trusted-clients"), trust_lines(trusted))?;
                let server_conf = dir.join("server.conf");
                std::fs::write(&server_conf, server_config(me, clients))?;
                let settings = dir.join("Deskflow.conf");
                std::fs::write(
                    &settings,
                    format!(
                        "[core]\nscreenName={me}\nport={PORT}\nwlClipboard=false\npreventSleep=false\n\
                         [server]\nexternalConfig=true\nexternalConfigFile={}\n\
                         [security]\ntlsEnabled=true\ncheckPeerFingerprints=true\ncertificate={}\n\
                         [log]\nlevel={log}\n",
                        server_conf.display(),
                        cert.display()
                    ),
                )?;
                Ok(("server", settings))
            }
            Want::Client { me, host, port, trusted_server } => {
                let dir = self.root.join("client");
                std::fs::create_dir_all(dir.join("tls"))?;
                std::fs::write(dir.join("tls/trusted-servers"), trust_lines(std::slice::from_ref(trusted_server)))?;
                let settings = dir.join("Deskflow.conf");
                // Keep the portal restore token Deskflow saved last time (no re-prompt).
                let token = std::fs::read_to_string(&settings)
                    .ok()
                    .and_then(|s| s.lines().find(|l| l.starts_with("xdpRestoreToken=")).map(str::to_owned))
                    .unwrap_or_default();
                std::fs::write(
                    &settings,
                    format!(
                        "[core]\nscreenName={me}\nport={port}\nwlClipboard=false\n\
                         [client]\nremoteHost={host}\n{token}\n\
                         [security]\ntlsEnabled=true\ncheckPeerFingerprints=true\ncertificate={}\n\
                         [log]\nlevel={log}\n",
                        cert.display()
                    ),
                )?;
                Ok(("client", settings))
            }
        }
    }
}

/// logind's graphical session of this user (object path), if any.
pub(crate) async fn graphical_session() -> Option<String> {
    let conn = zbus::Connection::system().await.ok()?;
    let user = zbus::Proxy::new(&conn, "org.freedesktop.login1", "/org/freedesktop/login1/user/self", "org.freedesktop.login1.User").await.ok()?;
    let (_, path): (String, zbus::OwnedObjectPath) = user.get_property("Display").await.ok()?;
    (path.as_str() != "/").then(|| path.to_string())
}

fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

fn trust_lines(fps: &[String]) -> String {
    fps.iter().map(|f| format!("v2:sha256:{}\n", f.to_lowercase())).collect()
}

/// Deskflow 1.26 text config: screens, links (with edge ranges), no clipboard.
fn server_config(me: &str, clients: &BTreeMap<String, Placement>) -> String {
    let mut s = String::from("section: screens\n");
    s += &format!("\t{me}:\n");
    for c in clients.keys() {
        s += &format!("\t{c}:\n");
    }
    s += "end\nsection: links\n";
    s += &format!("\t{me}:\n");
    for (c, p) in clients {
        s += &format!("\t\t{}{} = {c}\n", p.side.as_str(), range(p));
    }
    for (c, p) in clients {
        s += &format!("\t{c}:\n\t\t{} = {me}{}\n", p.side.opposite().as_str(), range(p));
    }
    s += "end\nsection: options\n\tclipboardSharing = false\nend\n";
    s
}

fn range(p: &Placement) -> String {
    if p.from == 0 && p.to >= 100 { String::new() } else { format!("({},{})", p.from, p.to.min(100)) }
}

/// Forwards deskflow-core's output into our log.
type Flag = std::sync::Arc<std::sync::atomic::AtomicBool>;

type Retry = std::sync::Arc<tokio::sync::Notify>;

fn forward_output(child: &mut tokio::process::Child, declined: Flag, retry: Retry) {
    if let Some(out) = child.stdout.take() {
        tokio::spawn(log_lines(out, declined.clone(), retry.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(log_lines(err, declined, retry));
    }
}

async fn log_lines(pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static, declined: Flag, retry: Retry) {
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(l)) = lines.next_line().await {
        // "failed to initialize input capture session, quitting: CreateSession canceled"
        if l.contains("input capture session") && l.contains("canceled") {
            declined.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        // Client side: "failed to initialize remote desktop session: CreateSession failed"
        // (GNOME inhibits new sessions while the screen is locked). The process
        // stays up without being able to move anything.
        if l.contains("failed to initialize remote desktop session") {
            retry.notify_one();
        }
        tracing::info!(target: "turad::deskflow::core", "{l}");
    }
}

fn spawn(mode: &'static str, settings: &Path, declined: Flag, retry: Retry) -> Result<tokio::process::Child> {
    let mut child = tokio::process::Command::new("deskflow-core")
        .arg(mode)
        .arg("--new-instance")
        .arg("-s")
        .arg(settings)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting deskflow-core (is the deskflow package installed?)")?;
    tracing::info!("deskflow-core {mode} started");
    forward_output(&mut child, declined, retry);
    Ok(child)
}

/// Saved placements of joined PCs (`tura layout`), by screen name.
#[derive(Default, Serialize, Deserialize)]
pub struct Layouts(BTreeMap<String, Placement>);

impl Layouts {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("layout.json")
    }

    pub fn load(config_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(config_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        std::fs::write(Self::path(config_dir), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = (String, Placement)> + '_ {
        self.0.iter().map(|(k, v)| (k.clone(), *v))
    }

    pub fn get(&self, screen: &str) -> Placement {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(screen)).map(|(_, p)| *p).unwrap_or_default()
    }

    pub fn set(&mut self, screen: &str, p: Placement) {
        self.0.retain(|k, _| !k.eq_ignore_ascii_case(screen));
        self.0.insert(screen.to_owned(), p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_with_range() {
        let mut c = BTreeMap::new();
        c.insert("Laptop".to_owned(), Placement { side: Side::Down, from: 0, to: 50 });
        let conf = server_config("Desk", &c);
        assert!(conf.contains("\tDesk:\n\t\tdown(0,50) = Laptop\n"), "{conf}");
        assert!(conf.contains("\tLaptop:\n\t\tup = Desk(0,50)\n"), "{conf}");
        assert!(conf.contains("clipboardSharing = false"));
    }

    #[test]
    fn full_edge_has_no_range() {
        let mut c = BTreeMap::new();
        c.insert("B".to_owned(), Placement { side: Side::Left, from: 0, to: 100 });
        let conf = server_config("A", &c);
        assert!(conf.contains("\t\tleft = B\n") && conf.contains("\t\tright = A\n"), "{conf}");
    }

    #[test]
    fn trust_line_format() {
        assert_eq!(trust_lines(&["ABCD".into()]), "v2:sha256:abcd\n");
    }
}
