# Debian and KDE Plasma setup

KDE Rat is the Debian/KDE downstream of Tunnelrat v0.4.0. The Android app and wire protocol remain upstream; the PC setup, clipboard integration, power actions, and session startup are adapted for Plasma.

## Install

Run the setup from the repository as your regular user:

```sh
./pc/setup.sh
```

The setup installs available Debian packages, builds `tura` and `turad` from the included Rust source, installs the desktop app and viewer, enables the user service, and configures a Plasma autostart helper to import display-session variables. `--no-system` skips package, group, polkit, udev, and Wake-on-LAN system changes; `--no-phone` skips ADB detection.

Rust 1.85 or newer is required. A full setup may request `sudo` to install packages and system rules. Log out and back in after group membership changes.

## KDE support

| Feature | Debian/KDE behavior |
| --- | --- |
| Clipboard | Uses `wl-clipboard` on Wayland and `xclip` on X11. Empty clipboard reads are treated as empty, not as backend failure. |
| PC screen to phone | Uses the XDG ScreenCast/RemoteDesktop portal with PipeWire and GStreamer. Plasma may ask you to select a screen and approve capture. |
| Phone screen to PC | Uses the bundled GTK/GStreamer viewer; this direction was observed streaming from a Samsung Galaxy S21 in the previous v0.3.0 installation. v0.4.0 still needs a full live check on KDE. |
| Screen power | Uses `kscreen-doctor --dpms off` on KDE Wayland or `xset dpms force off` on KDE X11. |
| Startup | A lingering user service starts the daemon at boot; the Plasma autostart helper imports desktop-session variables and restarts it when you log in. |
| GNOME top-bar chips | Not provided in Plasma. Manage paired devices with the desktop app or `tura devices`. |
| Shared mouse and keyboard | Requires Deskflow plus compatible KDE portal/input capture support. |

Check missing runtime dependencies and device readiness with:

```sh
~/.local/bin/tura status
~/.local/bin/tura devices
systemctl --user status turad
```

Screen capture still requires the user to accept the normal Android and Plasma permission prompts. See [CONTRIBUTORS.md](CONTRIBUTORS.md) and [LICENSE](LICENSE); the project is GPL-3.0-only. Bundled fonts keep their SIL Open Font License notices.
