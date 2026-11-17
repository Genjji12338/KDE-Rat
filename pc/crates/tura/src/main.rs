//! tura — Tunnelrat command-line tool.
//!
//! Lets you pair a phone (`tura pair`), test the daemon without the phone
//! (`tura power lock`, `tura move 50 0`) and wake other machines
//! (`tura wake aa:bb:..`). Talks to turad over its owner-only Unix socket.

use std::net::{Ipv4Addr, UdpSocket};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tura_proto::{DeviceKind, Message, MouseButton, PROTOCOL_VERSION, PowerAction, PressState};

#[derive(Parser)]
#[command(name = "tura", version, about = "Tunnelrat command-line tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show a QR code to pair a phone (valid for 5 minutes, one use).
    Pair,
    /// List paired phones.
    Devices,
    /// Revoke a phone: it's disconnected at once and must pair again.
    Unpair {
        /// Device name, or the start of its fingerprint (≥ 4 hex chars, see `tura devices`).
        device: String,
    },
    /// Show what the daemon reports (hostname, Wake-on-LAN MACs).
    Info,
    /// Checklist: what works on this PC, and how to fix what doesn't.
    Status,
    /// Shutdown, reboot, suspend, lock or blank a PC: this one, or `--device`.
    Power {
        action: Power,
        /// Another connected device by name (e.g. the PC you joined, or one that joined you).
        #[arg(long)]
        device: Option<String>,
    },
    /// Join another PC: paste the code its `tura pair` printed.
    Join {
        /// The tunnelrat://pair?… code.
        code: String,
    },
    /// Turn shared mouse/keyboard back on after GNOME's input-capture dialog was declined.
    ShareInput,
    /// Where a joined PC sits, for the shared mouse/keyboard.
    Layout {
        /// The other PC's name (see `tura devices`).
        device: String,
        /// Which edge of THIS screen leads to it.
        side: SideArg,
        /// Part of that edge in percent, e.g. 0,50 = first half (left/top).
        #[arg(long, value_parser = parse_range, default_value = "0,100")]
        range: (u8, u8),
    },
    /// Show a phone's screen on this PC (the phone starts mirroring).
    Mirror {
        /// The phone's name (see `tura devices`).
        device: String,
    },
    /// Ring a phone so you can find it (loud, even on silent).
    Ring {
        /// The phone's name (see `tura devices`).
        device: String,
    },
    /// View and control another PC's screen (opens Remmina).
    Screen {
        /// The other PC's name (see `tura devices`).
        device: String,
    },
    /// Move the pointer by dx, dy pixels.
    Move {
        #[arg(allow_hyphen_values = true)]
        dx: f32,
        #[arg(allow_hyphen_values = true)]
        dy: f32,
    },
    /// Click a mouse button.
    Click {
        #[arg(default_value = "left")]
        button: Button,
    },
    /// Send a Wake-on-LAN magic packet.
    Wake {
        /// Target MAC, e.g. aa:bb:cc:dd:ee:ff
        mac: String,
        /// Broadcast address.
        #[arg(long, default_value = "255.255.255.255")]
        broadcast: Ipv4Addr,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SideArg {
    Left,
    Right,
    Up,
    Down,
}

fn parse_range(s: &str) -> Result<(u8, u8), String> {
    let (a, b) = s.split_once(',').ok_or("use FROM,TO, e.g. 0,50")?;
    let (a, b): (u8, u8) = (a.trim().parse().map_err(|_| "bad FROM")?, b.trim().parse().map_err(|_| "bad TO")?);
    if a >= b || b > 100 {
        return Err("need 0 ≤ FROM < TO ≤ 100".into());
    }
    Ok((a, b))
}

#[derive(Clone, Copy, ValueEnum)]
enum Power {
    Shutdown,
    Reboot,
    Suspend,
    Lock,
    ScreenOff,
}

#[derive(Clone, Copy, ValueEnum)]
enum Button {
    Left,
    Right,
    Middle,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Wake { mac, broadcast } => wake(&mac, broadcast),
        Cmd::Devices => {
            for r in send(Some(Message::ListDevices)).await? {
                match r {
                    Message::Devices { devices, .. } if devices.is_empty() => println!("No paired phones. Run `tura pair` to add one."),
                    Message::Devices { devices, .. } => {
                        println!("{:<20} {:<14} {:<10} {:<10} PAIRED", "NAME", "FINGERPRINT", "STATUS", "BATTERY");
                        for d in devices {
                            let status = if d.online > 0 { "online" } else { "offline" };
                            let battery = d.battery.map(|b| format!("{b}%{}", if d.charging { " ⚡" } else { "" })).unwrap_or_else(|| "–".into());
                            println!("{:<20} {:<14} {:<10} {:<10} {}", d.name, &d.fingerprint[..12.min(d.fingerprint.len())], status, battery, ago(d.paired_at));
                        }
                    }
                    Message::Error { message } => bail!("daemon: {message}"),
                    _ => {}
                }
            }
            Ok(())
        }
        Cmd::Unpair { device } => {
            for r in send(Some(Message::Unpair { selector: device })).await? {
                match r {
                    Message::Unpaired { device } => {
                        let note = if device.online > 0 { " and disconnected it" } else { "" };
                        println!("Unpaired {} ({}){note}.", device.name, &device.fingerprint[..12.min(device.fingerprint.len())]);
                    }
                    Message::Error { message } => bail!("{message}"),
                    _ => {}
                }
            }
            Ok(())
        }
        Cmd::Pair => {
            for r in send(Some(Message::PairBegin)).await? {
                match r {
                    Message::PairOffer { uri, expires_in_secs, .. } => {
                        let code = qrcode::QrCode::new(uri.as_bytes())?;
                        let qr = code
                            .render::<qrcode::render::unicode::Dense1x2>()
                            .dark_color(qrcode::render::unicode::Dense1x2::Light)
                            .light_color(qrcode::render::unicode::Dense1x2::Dark)
                            .quiet_zone(true)
                            .build();
                        println!("{qr}");
                        println!("Phone: scan with Tunnelrat → Pair.");
                        println!("PC:    tura join '{uri}'");
                        println!("Valid for {} min, one device.", expires_in_secs / 60);
                    }
                    Message::Error { message } => bail!("daemon: {message}"),
                    _ => {}
                }
            }
            Ok(())
        }
        Cmd::Status => {
            let mut all_ok = true;
            for r in send(Some(Message::GetStatus)).await? {
                match r {
                    Message::Status { items } => {
                        for i in items {
                            all_ok &= i.ok;
                            println!("{} {:<30} {}", if i.ok { "✓" } else { "✗" }, i.name, i.detail);
                        }
                    }
                    Message::Error { message } => bail!("daemon: {message}"),
                    _ => {}
                }
            }
            if !all_ok {
                std::process::exit(1);
            }
            Ok(())
        }
        Cmd::Info => {
            let replies = send(None).await?;
            for r in replies {
                if let Message::PcInfo { hostname, wol_macs, power_actions, .. } = r {
                    println!("hostname: {hostname}");
                    println!("wol macs: {}", if wol_macs.is_empty() { "(none wired)".into() } else { wol_macs.join(", ") });
                    println!("power:    {power_actions:?}");
                }
            }
            Ok(())
        }
        Cmd::Join { code } => {
            report(send(Some(Message::JoinPc { uri: code })).await?)?;
            println!("Joining… check `journalctl --user -u turad` or `tura devices` on the other PC.");
            Ok(())
        }
        Cmd::Layout { device, side, range: (from, to) } => {
            let side = match side {
                SideArg::Left => tura_proto::Side::Left,
                SideArg::Right => tura_proto::Side::Right,
                SideArg::Up => tura_proto::Side::Up,
                SideArg::Down => tura_proto::Side::Down,
            };
            report(send(Some(Message::SetLayout { device, side, from, to })).await?)
        }
        Cmd::Power { action, device } => {
            let action = match action {
                Power::Shutdown => PowerAction::Shutdown,
                Power::Reboot => PowerAction::Reboot,
                Power::Suspend => PowerAction::Suspend,
                Power::Lock => PowerAction::Lock,
                Power::ScreenOff => PowerAction::ScreenOff,
            };
            let msg = Message::Power { action };
            match device {
                Some(device) => report(send(Some(Message::ForDevice { device, message: Box::new(msg) })).await?),
                None => report(send(Some(msg)).await?),
            }
        }
        Cmd::ShareInput => {
            report(send(Some(Message::ShareInput)).await?)?;
            println!("GNOME will ask for input capture again.");
            Ok(())
        }
        Cmd::Mirror { device } => report(send(Some(Message::ForDevice { device, message: Box::new(Message::MirrorRequest) })).await?),
        Cmd::Ring { device } => {
            report(send(Some(Message::ForDevice { device: device.clone(), message: Box::new(Message::Ring) })).await?)?;
            println!("{device} is ringing; stop it on the phone.");
            Ok(())
        }
        Cmd::Screen { device } => {
            println!("Asking {device} to share its screen…");
            report(send(Some(Message::ScreenView { device })).await?)
        }
        Cmd::Move { dx, dy } => report(send(Some(Message::PointerMove { dx, dy })).await?),
        Cmd::Click { button } => {
            let button = match button {
                Button::Left => MouseButton::Left,
                Button::Right => MouseButton::Right,
                Button::Middle => MouseButton::Middle,
            };
            report(send(Some(Message::PointerButton { button, state: PressState::Click })).await?)
        }
    }
}

/// Connects, says hello, optionally sends one message, then collects replies
/// until a ping round-trip confirms the daemon processed everything.
async fn send(msg: Option<Message>) -> Result<Vec<Message>> {
    let path = tura_proto::admin_socket_path();
    let stream = tokio::net::UnixStream::connect(&path)
        .await
        .with_context(|| format!("cannot reach turad at {} — is it running? (systemctl --user status turad)", path.display()))?;
    let (ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream).await?;
    let (mut tx, mut rx) = ws.split();
    let out = |m: Message| WsMessage::Text(m.to_json().into());

    tx.send(out(Message::Hello { protocol: PROTOCOL_VERSION, device_name: "tura-cli".into(), device_kind: DeviceKind::Pc }))
        .await?;
    if let Some(m) = msg {
        tx.send(out(m)).await?;
    }
    tx.send(out(Message::Ping)).await?;

    let mut replies = vec![];
    while let Some(frame) = rx.next().await {
        if let WsMessage::Text(t) = frame? {
            match Message::from_json(&t)? {
                Message::Pong => break,
                m => replies.push(m),
            }
        }
    }
    let _ = tx.send(WsMessage::Close(None)).await;
    Ok(replies)
}

/// "3 min ago", "2 days ago" for a unix timestamp.
fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(unix);
    let s = now.saturating_sub(unix);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

fn report(replies: Vec<Message>) -> Result<()> {
    for r in replies {
        if let Message::Error { message } = r {
            bail!("daemon: {message}");
        }
    }
    println!("ok");
    Ok(())
}

fn wake(mac: &str, broadcast: Ipv4Addr) -> Result<()> {
    let bytes: Vec<u8> = mac
        .split([':', '-'])
        .map(|h| u8::from_str_radix(h, 16))
        .collect::<Result<_, _>>()
        .context("MAC must look like aa:bb:cc:dd:ee:ff")?;
    if bytes.len() != 6 {
        bail!("MAC must have 6 bytes");
    }
    let mut packet = vec![0xFFu8; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&bytes);
    }
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    sock.send_to(&packet, (broadcast, 9))?;
    println!("magic packet sent to {mac}");
    Ok(())
}
