#!/bin/sh
# Tunnelrat setup for Debian with KDE Plasma. Run from this bundle:
#
#   pc/setup.sh              everything
#   pc/setup.sh --no-system  skip the parts that need sudo (packages, rules)
#   pc/setup.sh --no-phone   skip the phone step even if one is plugged in
#
# Safe to run again (e.g. to update): it installs what's missing, replaces
# Tunnelrat's own files and leaves everything else alone. Asks for your
# password once, for the system parts.
#
# SPDX-License-Identifier: GPL-3.0-only
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
bin="$HOME/.local/bin"
system=1
phone=1
for arg in "$@"; do
    case "$arg" in
        --no-system) system=0 ;;
        --no-phone) phone=0 ;;
        -h | --help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg (see --help)" >&2; exit 2 ;;
    esac
done

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
note() { printf '   %s\n' "$*"; }
warn() { printf '   \033[33m! %s\033[0m\n' "$*"; }
has() { command -v "$1" >/dev/null 2>&1; }
installed() { dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q "install ok installed"; }
available() { apt-cache show "$1" >/dev/null 2>&1; }

[ "$(id -u)" -ne 0 ] || { echo "Run this as yourself, not with sudo: it asks for your password when it needs it." >&2; exit 1; }
has apt-get || { echo "Debian or Ubuntu with apt is required." >&2; exit 1; }
if [ -r /etc/os-release ]; then
    . /etc/os-release
    case "${ID:-}" in debian|ubuntu|linuxmint|neon) ;; *) warn "Untested distribution: ${PRETTY_NAME:-unknown}; continuing because apt is available." ;; esac
fi
case ":${XDG_CURRENT_DESKTOP:-}:" in
    *:KDE:*) ;;
    *) warn "KDE Plasma was not detected; the KDE portal backend may not be installed." ;;
esac
relogin=0

# ── 1. Packages ───────────────────────────────────────────────────────────
if [ "$system" = 1 ]; then
    step "Packages"
    want="gstreamer1.0-tools gstreamer1.0-pipewire gstreamer1.0-plugins-base gstreamer1.0-plugins-good
          gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-gtk4 python3-gi gir1.2-gtk-4.0 gir1.2-adw-1
          gir1.2-gstreamer-1.0 gir1.2-gst-plugins-base-1.0 gir1.2-gdkpixbuf-2.0 openssl fuse3 avahi-daemon
          libxkbcommon0 ethtool xdg-desktop-portal xdg-desktop-portal-kde wl-clipboard"
    # Deskflow is optional. Tunnelrat needs the upstream deskflow-core binary;
    # Debian trixie currently ships the incompatible legacy deskflow-server/client
    # interface, so do not install it as a false fix.
    if ! has deskflow-core; then
        warn "Shared mouse/keyboard needs deskflow-core. Install the current Debian build from https://github.com/deskflow/deskflow/releases."
    fi
    if available xclip; then want="$want xclip"; fi
    if available libkscreen-bin; then want="$want libkscreen-bin"; fi
    if available x11-xserver-utils; then want="$want x11-xserver-utils"; fi
    # Hardware video encoding (VA-API), where the distribution ships it.
    for opt in gstreamer1.0-plugins-extra gstreamer1.0-plugins-bad; do
        available "$opt" && want="$want $opt"
    done
    missing=""
    for p in $want; do
        installed "$p" || missing="$missing $p"
    done
    if [ -n "$missing" ]; then
        note "Installing:$missing"
        sudo apt-get update -qq
        # shellcheck disable=SC2086
        sudo apt-get install -y $missing
    else
        note "All there."
    fi
fi

# ── 2. Programs ───────────────────────────────────────────────────────────
step "Tunnelrat programs"
if [ -f "$here/Cargo.toml" ] && { has cargo || [ -x "$HOME/.cargo/bin/cargo" ]; }; then
    cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
    note "Building the adapted source (cargo build --release)…"
    (cd "$here" && "$cargo" build --release --locked --quiet)
    src="$here/target/release"
else
    echo "Rust 1.85 or newer is required to build the included edition-2024 source workspace." >&2
    echo "Install a current Rust toolchain, then run this setup again." >&2
    exit 1
fi
mkdir -p "$bin"
for f in turad tura; do
    # Replace atomically: the running turad keeps its old file until restarted.
    install -m 755 "$src/$f" "$bin/.$f.new" && mv -f "$bin/.$f.new" "$bin/$f"
done
install -m 755 "$here/viewer/tura-viewer" "$bin/tura-viewer"
sh "$here/app/install.sh" >/dev/null
note "Installed turad, tura, tura-viewer and the Tunnelrat app in $bin"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) warn "$bin isn't on your PATH yet; add it to PATH or log in again." ;;
esac

# ── 3. Service ────────────────────────────────────────────────────────────
step "Background service"
unit="$HOME/.config/systemd/user/turad.service"
mkdir -p "$(dirname "$unit")"
if [ -f "$unit" ] && ! cmp -s "$here/packaging/turad.service" "$unit"; then
    cp "$unit" "$unit.bak"
    note "Saved your previous unit as $unit.bak"
fi
cp "$here/packaging/turad.service" "$unit"
systemctl --user daemon-reload
systemctl --user enable turad >/dev/null 2>&1
systemctl --user restart turad
note "turad runs as a user service (systemctl --user status turad)"
if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" != "yes" ]; then
    if loginctl enable-linger "$USER" 2>/dev/null || { [ "$system" = 1 ] && sudo loginctl enable-linger "$USER"; }; then
        note "Runs from boot: reachable before you log in (Wake on LAN, log in from the phone)"
    else
        warn "Couldn't enable starting at boot (loginctl enable-linger)"
    fi
fi

# ── 4. Desktop integration ────────────────────────────────────────────────
# Environment inherited by a lingering user service at boot may not contain
# graphical-session variables. Import them and restart turad at Plasma login.
step "KDE Plasma integration"
mkdir -p "$HOME/.config/autostart" "$HOME/.local/bin"
session_start="$HOME/.local/bin/tunnelrat-session-start"
cat > "$session_start" <<'EOF'
#!/bin/sh
systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XAUTHORITY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP 2>/dev/null || true
systemctl --user restart turad 2>/dev/null || true
EOF
chmod 755 "$session_start"
cat > "$HOME/.config/autostart/tunnelrat-session.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Tunnelrat session setup
Comment=Connect Tunnelrat to the Plasma clipboard and desktop session
Exec=$session_start
X-KDE-autostart-after=panel
EOF
note "Plasma login imports the display variables and restarts turad for clipboard access."
note "The KDE portal handles screen sharing where its RemoteDesktop features are supported."
note "GNOME top-bar device chips are not available in Plasma."

# ── 5. System rules ───────────────────────────────────────────────────────
if [ "$system" = 1 ]; then
    step "Permissions (power actions, typing at the login screen)"
    for g in tunnelrat uinput; do
        sudo groupadd -f "$g"
        if ! id -nG "$USER" | tr ' ' '\n' | grep -qx "$g"; then
            sudo usermod -aG "$g" "$USER"
            relogin=1
        fi
    done
    rules_changed=0
    copy_rule() { # file, dest dir
        if ! cmp -s "$here/packaging/$1" "$2/$1"; then
            sudo install -m 644 "$here/packaging/$1" "$2/$1"
            rules_changed=1
        fi
    }
    copy_rule 50-tunnelrat-power.rules /etc/polkit-1/rules.d
    copy_rule 60-tunnelrat-uinput.rules /etc/udev/rules.d
    if [ "$rules_changed" = 1 ]; then
        sudo udevadm control --reload
        sudo udevadm trigger --name-match=uinput
    fi
    note "Power actions without a password prompt; /dev/uinput for the uinput group"

    step "Wake on LAN"
    wired=$(nmcli -t -f NAME,TYPE,DEVICE connection show --active 2>/dev/null | awk -F: '$2 == "802-3-ethernet" { print; exit }')
    if [ -z "$wired" ]; then
        note "No wired connection: Wake on LAN needs Ethernet (skipped)"
    else
        name=${wired%%:*}
        dev=${wired##*:}
        if [ "$(nmcli -g 802-3-ethernet.wake-on-lan connection show "$name")" != "magic" ]; then
            sudo nmcli connection modify "$name" 802-3-ethernet.wake-on-lan magic
        fi
        sudo ethtool -s "$dev" wol g 2>/dev/null || warn "Couldn't turn on Wake on LAN on $dev now; it applies on the next connect"
        note "$dev wakes the PC on a magic packet (also check the BIOS: Wake on LAN on, ErP off)"
    fi
fi

# ── 6. Phone ──────────────────────────────────────────────────────────────
adb=$(command -v adb || echo "$HOME/Android/Sdk/platform-tools/adb")
if [ "$phone" = 1 ] && [ -x "$adb" ] && "$adb" get-state >/dev/null 2>&1; then
    step "Phone (plugged in over USB)"
    apk=$(ls -t "$repo"/android/app/build/outputs/apk/release/*.apk "$repo"/android/app/build/outputs/apk/debug/*.apk 2>/dev/null | head -1 || true)
    if [ -n "$apk" ]; then
        "$adb" install -r "$apk" >/dev/null && note "Installed the app ($(basename "$apk"))"
    fi
    if "$adb" shell pm path dev.tunnelrat >/dev/null 2>&1; then
        note "Screen capture uses Android's standard approval prompt"
        note "For control from the PC: phone Settings → Accessibility → Tunnelrat: control from PC"
    else
        note "Tunnelrat is not installed on the phone. Install it from the project's F-Droid repository or releases, then reconnect over USB to enable prompt-free mirroring."
    fi
fi

# ── Done ──────────────────────────────────────────────────────────────────
step "Status"
sleep 2
"$bin/tura" status || true
echo
if [ "$relogin" = 1 ]; then
    printf '\033[1mLog out and back in once\033[0m (new group membership takes effect then).\n'
fi
echo "Pair a phone: open Tunnelrat from the app grid → Pair a device, and scan the code in the phone app."
echo "Join another PC to this one: Pair a device here → Copy code, then run it on the other PC."
