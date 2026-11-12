#!/bin/sh
# Tunnelrat setup for this PC (Ubuntu with GNOME). Run from the repository:
#
#   pc/setup.sh              everything
#   pc/setup.sh --no-system  skip the parts that need sudo (packages, rules)
#   pc/setup.sh --no-phone   skip the phone step even if one is plugged in
#
# Safe to run again (e.g. to update): it installs what's missing, replaces
# Tunnelrat's own files and leaves everything else alone. Asks for your
# password once, for the system parts.
#
# SPDX-License-Identifier: GPL-3.0-or-later
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
has apt-get || { echo "This setup is for Ubuntu (apt)." >&2; exit 1; }
case "${XDG_CURRENT_DESKTOP:-}" in
    *GNOME*) ;;
    *) warn "This isn't a GNOME session: clipboard, top-bar chips and screen sharing need GNOME." ;;
esac
relogin=0

# ── 1. Packages ───────────────────────────────────────────────────────────
if [ "$system" = 1 ]; then
    step "Packages"
    want="deskflow gstreamer1.0-tools gstreamer1.0-pipewire gstreamer1.0-plugins-base gstreamer1.0-plugins-good
          gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-gtk4 python3-gi gir1.2-gtk-4.0 gir1.2-adw-1
          gir1.2-gstreamer-1.0 gir1.2-gst-plugins-base-1.0 gir1.2-gdkpixbuf-2.0 openssl fuse3 avahi-daemon
          libxkbcommon0 ethtool"
    # Hardware video encoding (VA-API), where Ubuntu ships it.
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
if [ -x "$here/bin/turad" ]; then
    src="$here/bin" # a release bundle
elif has cargo || [ -x "$HOME/.cargo/bin/cargo" ]; then
    cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
    note "Building (cargo build --release)…"
    (cd "$here" && "$cargo" build --release --quiet)
    src="$here/target/release"
else
    echo "No Rust to build with and no prebuilt binaries in $here/bin." >&2
    echo "Install Rust (https://rustup.rs) and run this again." >&2
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
    *) warn "$bin isn't on your PATH yet; it is after logging in again (Ubuntu's ~/.profile adds it)." ;;
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

# ── 4. GNOME extension ────────────────────────────────────────────────────
step "GNOME extension (clipboard, top-bar chips)"
ext="tunnelrat@tunnelrat.dev"
dest="$HOME/.local/share/gnome-shell/extensions/$ext"
mkdir -p "$dest"
cp -r "$here/gnome-extension/$ext/." "$dest/"
if ! gnome-extensions enable "$ext" 2>/dev/null; then
    # GNOME only knows new extensions after logging in again: pre-enable it.
    if has gsettings; then
        python3 - "$ext" <<'EOF'
import ast, subprocess, sys
ext = sys.argv[1]
cur = subprocess.run(["gsettings", "get", "org.gnome.shell", "enabled-extensions"], capture_output=True, text=True).stdout.strip()
lst = ast.literal_eval(cur.replace("@as ", "")) if cur else []
if ext not in lst:
    subprocess.run(["gsettings", "set", "org.gnome.shell", "enabled-extensions", str(lst + [ext])], check=True)
EOF
    fi
    relogin=1
fi
note "Installed and enabled"

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
        "$adb" shell appops set dev.tunnelrat PROJECT_MEDIA allow
        note "Showing the phone on the PC starts without Android's prompt"
        note "For control from the PC: phone Settings → Accessibility → Tunnelrat: control from PC"
    else
        warn "The Tunnelrat app isn't on the phone yet (build it: cd android && ./gradlew assembleDebug)"
    fi
fi

# ── Done ──────────────────────────────────────────────────────────────────
step "Status"
sleep 2
"$bin/tura" status || true
echo
if [ "$relogin" = 1 ]; then
    printf '\033[1mLog out and back in once\033[0m (new groups and the GNOME extension take effect then).\n'
fi
echo "Pair a phone: open Tunnelrat from the app grid → Pair a device, and scan the code in the phone app."
echo "Join another PC to this one: Pair a device here → Copy code, then run it on the other PC."
