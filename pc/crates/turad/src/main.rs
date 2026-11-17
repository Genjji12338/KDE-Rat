//! turad — the Tunnelrat PC daemon.
//!
//! MVP scope: accept a phone connection, inject mouse/keyboard input through
//! uinput, run power actions through logind, and tell the phone our MAC for
//! Wake on LAN.
//!
//! SECURITY: phones must use mutual TLS and be paired (see `server`, `tls`,
//! `pairing`). There is no plaintext network listener; the local CLI talks
//! over an owner-only Unix socket.

mod bluetooth;
mod cast;
mod deskflow;
mod drive;
mod files;
mod gamepad;
mod identity;
mod input;
mod keymap;
mod mdns;
mod media;
mod mirror;
mod notify;
mod pairing;
mod peer;
mod power;

mod settings;
mod sleep;
mod server;
mod shell;
mod sysinfo;
mod tls;

use std::net::SocketAddr;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "turad", version, about = "Tunnelrat PC daemon")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value_t = SocketAddr::from(([0, 0, 0, 0], tura_proto::DEFAULT_PORT)))]
    listen: SocketAddr,

    /// Allow remote suspend. Off by default because some GPUs never come back from S3.
    #[arg(long)]
    allow_suspend: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "turad=info".into()))
        .init();

    let args = Args::parse();

    server::run(args.listen, power::Policy { allow_suspend: args.allow_suspend }).await
}
