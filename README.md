# Tunnelrat

Phone ↔ PC link: touchpad, power, Wake on LAN, and later the phone as a drive. See [PLAN.md](PLAN.md).

```
pc/        Rust workspace
  crates/tura-proto   wire protocol (JSON over WebSocket)
  crates/turad        PC daemon: uinput input, logind power, WoL info
  crates/tura         CLI: test the daemon, send WoL packets
  packaging/          udev rule, polkit rule, systemd user unit
android/   Kotlin + Jetpack Compose app (dev.tunnelrat)
```

## Install (phone)
In the F-Droid app: Settings → Repositories → **+**, and add

    https://i232826.github.io/tunnelrat-fdroid/repo?fingerprint=448AC2C9C7C73A1D920CD6CA7703B81ADDCF9FD198E8C40F1B4D9173FE9D32E9

(or scan the QR code on [that page](https://i232826.github.io/tunnelrat-fdroid/repo/)), then
install Tunnelrat. The APK is also on the [releases page](https://github.com/I232826/tunnelrat/releases);
both are signed with the same key, so either one updates the other.

## Install (PC)
From the repository (needs Rust: https://rustup.rs):
```bash
pc/setup.sh
```
Or on a PC without Rust or the source: build the bundle here with `pc/bundle.sh`,
copy `dist/tunnelrat-pc.tar.gz` over and run
`tar xzf tunnelrat-pc.tar.gz && tunnelrat/pc/setup.sh`.

The setup installs the packages it needs (Deskflow, GStreamer, GTK 4/libadwaita for
Python, Avahi, FUSE), turad, `tura`, the screen viewer and the Tunnelrat app, the
user service (running from boot), the GNOME extension, the power and uinput rules and
Wake on LAN on the wired connection. With the phone plugged in over USB it also
installs the app there and lets mirroring start without Android's prompt. It asks for
your password once, can be run again to update, and ends with `tura status`
(`--no-system` skips the sudo parts, `--no-phone` the phone). The sections below
describe the same steps by hand.

## Status
Working: QR pairing + mutual TLS over Wi-Fi (LAN first, then Tailscale), touchpad (move, tap, two-finger scroll and right-click), mouse buttons, Wake on LAN, lock, screen off, reboot and shutdown (Restart and Off need a hold). Suspend is opt-in (`turad --allow-suspend`).

## Phone notifications on the PC
App settings → show phone notifications on the PC (Android asks for notification
access). Messages with a reply field get a **Reply** button on the PC; it opens a small
Tunnelrat window, and the phone sends the text through the notification's own reply
action (the only action the PC can trigger).

## PC app
`pc/app/install.sh` installs **Tunnelrat** in the app grid (GTK 4 + libadwaita,
Python; needs `python3-gi` and libadwaita, which Ubuntu's desktop has):
- **Devices:** paired phones and PCs with status, battery and drive; pair with a QR
  code (or copy a `tura join` code for another PC); view a PC's screen, show a phone's
  screen here, power actions, open the phone's files, unpair.
- **Layout:** this PC's monitors to scale; drag a joined PC to the edge where the mouse
  crosses over (saved like `tura layout`).
- **Settings:** clipboard sync, shared mouse/keyboard, allow sleep, and which devices
  show as chips in the top bar.
- **Status:** a checklist of what works and how to fix what doesn't.

Clicking the device chips in the top bar opens quick actions and "Open Tunnelrat"
(after logging in again once, so GNOME loads the new extension).

## GNOME extension (clipboard + phone battery in the top bar)
Wayland doesn't let background programs read the clipboard, so a tiny extension bridges it for turad only (it checks the caller's executable).
```bash
cp -r pc/gnome-extension/tunnelrat@tunnelrat.dev ~/.local/share/gnome-shell/extensions/
gnome-extensions enable tunnelrat@tunnelrat.dev   # or add it to org.gnome.shell enabled-extensions
```
Log out and back in once (Wayland loads new extensions at login).

## Finding the PC
turad announces itself on the local network (mDNS via Avahi, `_tunnelrat._tcp`). If
the PC's address changes, the app finds it again by its certificate fingerprint and
remembers the new address. A PC joined with `tura join` does the same for the PC it
joined. The connection is still checked against the pinned certificate.

## Pairing
```bash
~/.cargo/bin/tura pair
```
Tap **Scan QR code** in the app (or scan with the camera app and tap the link). Codes are one-time and expire after 5 minutes. Paired phones are stored in `~/.config/tunnelrat/paired.json`; the PC identity is `~/.config/tunnelrat/turad-{cert,key}.pem`.

```bash
tura devices              # paired phones, online/offline
tura unpair "Pixel 7"     # or a fingerprint prefix; kicks it off immediately
```
"Unpair this PC" in the app removes the pairing on both sides.

The phone can be paired with several PCs (menu → **Pair another PC**) and stays
connected to all of them: whatever a PC starts (mirroring, the drive, files) works from
any of them. What you start on the phone (touchpad, power, media, screens) goes to the
PC selected in the status-pill menu; the clipboard syncs with that PC and with the one
the phone's screen is shown on. Battery and notifications go to all.

## Dev setup

```bash
export PATH="$HOME/.cargo/bin:$HOME/Android/Sdk/platform-tools:$PATH"
export ANDROID_HOME="$HOME/Android/Sdk"
```

PC daemon:
```bash
cd pc && cargo run -p turad
cargo run -p tura -- info          # hostname + WoL MAC
cargo run -p tura -- power lock
```

Phone (USB debugging on):
```bash
cd android && ./gradlew installDebug
```
On Android 17 the app needs the "Nearby devices" permission (`ACCESS_LOCAL_NETWORK`); without it LAN connections silently time out.

Power actions from a user service need the polkit rule (one time):
```bash
sudo groupadd -f tunnelrat && sudo usermod -aG tunnelrat $USER
sudo cp pc/packaging/50-tunnelrat-power.rules /etc/polkit-1/rules.d/
```

## PC screen on the phone
Menu (status pill) → **PC screen** shows the PC's primary monitor, no dialog on the PC
(turad uses GNOME's own screen-cast API, like GNOME Remote Desktop; GNOME's top bar
shows that the screen is shared). On other desktops it falls back to the portal,
which asks once. Drag to move
the pointer, tap to click, long-press for right click, long-press then drag to drag,
two fingers to scroll; the keyboard button opens the phone keyboard. Needs
`gstreamer1.0-pipewire` and an H.264 encoder (VA-API from `gstreamer1.0-plugins-bad`,
or x264 from `gstreamer1.0-plugins-ugly`). GNOME's top bar shows that the screen is
being shared; stopping it there ends it on the phone too.

## Phone screen on the PC
Menu → **Show phone on PC**, or from the PC: `tura mirror "Pixel 7"`. Android asks
each time unless you allow it once over USB (debugging on):
`adb shell appops set dev.tunnelrat PROJECT_MEDIA allow`. Starting from the PC while
the app is closed needs the control (accessibility) service below; otherwise a
notification asks you to tap. A window opens on the PC:
left mouse button = finger (tap, hold, drag live), right click / Esc = Back, middle
click = Home, wheel = swipe, typing goes to the focused field; the bar has Back,
Home, Recent apps and Notifications. Control needs Settings → Accessibility →
**Tunnelrat: control from PC** (used only while mirroring). Install the viewer next
to turad: `cp pc/viewer/tura-viewer ~/.cargo/bin/` (needs `python3-gi`, GTK 4 and
the `gtk4paintablesink` + `avdec_h264` GStreamer elements).

## Bluetooth (no network)
When the phone can't reach the PC over the network (no Wi-Fi, PC on another network) and
the PC has Bluetooth switched on, the app connects over Bluetooth instead: same pairing,
same encryption (TLS runs end to end), every feature except the PC's screen (too slow).
It moves back to Wi-Fi by itself once the PC answers there again. Set it up once per PC
while connected over Wi-Fi: phone app → Settings → Bluetooth → **Set up Bluetooth**, then
confirm Android's "Pair?" prompt. (The PC opens Bluetooth pairing for a minute, only for
this phone, and restores its Bluetooth settings afterwards.) Keep the phone's Bluetooth on.

## Gamepad mode
Controller icon on the phone's home screen (or the PC menu → Gamepad mode): the phone becomes
an Xbox 360 controller on the PC (virtual uinput device, one per phone), so games and Steam use
it without setup. Two sticks (the left one centres where your thumb lands; tap a stick for
L3/R3), d-pad, A/B/X/Y, LB/RB, LT/RT, View/Home/Menu; volume keys are LB/RB. Gyro aiming
(top bar) steers the right stick by turning the phone. The pencil opens the editor: drag any
control, tap it to resize, Done saves (per phone). Works over Bluetooth too, with a bit more lag.

## Find my phone, lock when I leave
- **Ring a phone:** bell button in the Tunnelrat app, "Ring it" in the top-bar menu, or
  `tura ring <phone>`. It rings at full alarm volume (also on silent) and vibrates for up to
  a minute; stop it from the notification or by opening the app.
- **Lock when I leave:** Tunnelrat app → Settings → Lock when I leave, per phone. The PC locks
  once that phone has been gone for 30 s (out of Wi-Fi and Bluetooth range, or switched off;
  noticing a phone vanished can take up to ~45 s more). Not after the PC slept, not at the login screen.

## Log in from the phone (after Wake on LAN)
turad runs from boot (user lingering), so a woken PC is reachable before anyone logs
in. The app then shows "<PC> is at the login screen" with a password field; turad
types it into GNOME's login screen (first user) and reports whether the desktop
started. That also unlocks the keyring. Needs the uinput rule, since at the login
screen your user has no access to `/dev/uinput` otherwise:
```bash
sudo groupadd -f uinput && sudo usermod -aG uinput $USER
sudo cp pc/packaging/60-tunnelrat-uinput.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput
```

## PC ↔ PC (shared mouse/keyboard, clipboard, power, screen)
A second Ubuntu PC joins the first one the way a phone does, over the same
paired, mutual-TLS link:

```bash
tura pair                                   # on the main PC: prints a code
tura join 'tunnelrat://pair?…'              # on the other PC
tura layout <other-pc> down --range 0,50    # main PC: where it sits (edge + part of it)
tura power lock --device <other-pc>         # any power action, on either side
```

- **Shared mouse/keyboard** uses [Deskflow](https://github.com/deskflow/deskflow)
  (`sudo apt install deskflow` on both). turad writes Deskflow's config, trusts the
  other PC's Deskflow certificate via the Tunnelrat link, and runs `deskflow-core`
  as a separate process; it's restarted after a logout/login. GNOME asks for input capture on the
  main PC whenever Deskflow starts (it can't remember that permission): turad keeps
  Deskflow running when the other PC's link drops, and after a decline stays off
  until `tura share-input`.
- **Clipboard** is one shared clipboard across all devices (phones and PCs); each
  PC needs the GNOME extension.
- **Battery**: a laptop's battery shows as its own chip in the top bar.
- **Screen**: `tura screen <other-pc>` (or the app / top-bar menu) shows the other PC's
  screen in a window, with mouse and keyboard. The other PC casts its primary monitor
  through GNOME's own screen-cast API (no dialog; GNOME shows its sharing indicator),
  with its pointer drawn into the picture, over the Tunnelrat link; the window is
  `tura-viewer` in PC mode. Shortcuts GNOME itself handles (Super, Alt+Tab) stay on
  the viewing PC.
- Files between PCs: use SFTP in GNOME Files (`sftp://<other-pc>/`), no Tunnelrat
  needed.

## Releases and F-Droid
- **APK:** `android/release.sh` builds the signed release into `dist/`. The signing key
  is outside the repository (`~/.android-keys/`, see `android/app/build.gradle.kts`):
  back it up, without it no update can replace an installed release.
- **F-Droid:** store texts, icon, screenshots and changelogs are in
  `fastlane/metadata/android/en-US/` (F-Droid reads them from the tagged commit). Each
  release needs a `vX.Y.Z` tag matching `versionName`, and a `changelogs/<versionCode>.txt`.
  `fdroid/dev.tunnelrat.yml` is the draft build recipe for a merge request to
  [fdroiddata](https://gitlab.com/fdroid/fdroiddata): fill in the public repository URL,
  test it with F-Droid's tools (`fdroid build -v -l dev.tunnelrat` in an fdroiddata
  checkout), then open the merge request.
- **Own F-Droid repository:** `fdroid/update-repo.sh` adds the APKs from `dist/` to the
  repository at https://github.com/I232826/tunnelrat-fdroid (working copy with its index
  signing key in `~/tunnelrat-fdroid/`, never pushed: back it up too) and pushes it.

## License
Tunnelrat (PC daemon, CLI, GNOME extension and Android app) is licensed under the
**GNU General Public License v3.0 or later**, see [LICENSE](LICENSE).

All bundled dependencies are permissively licensed (MIT / Apache-2.0 / BSD / ISC,
which are GPL-3.0-compatible). Shared mouse/keyboard uses
[Deskflow](https://github.com/deskflow/deskflow) (GPL-2.0-only with an OpenSSL
exception) as a **separate program** that turad configures and starts; no Deskflow
code is part of Tunnelrat. Note that GPL-2.0-only code cannot be merged into this
GPL-3.0 code base, so Deskflow must stay a separate process.
