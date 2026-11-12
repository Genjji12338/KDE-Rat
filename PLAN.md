# Tunnelrat

Connects an Android phone to an Ubuntu PC: remote control, power, and file access. It doesn't use KDE Connect's protocol and has its own look.
CLI / short tag: `tura`.

## Decisions
- **Phone:** Android only. Kotlin + Jetpack Compose. Custom design: dark, bold, card-based, big touch targets.
- **PC:** Rust daemon (`turad`) running as a systemd user service, plus the `tura` CLI.
- **Target PC:** Ubuntu with GNOME on Wayland.

## Architecture

### Phone
- **Phone as a drive:** a WebDAV server announced over mDNS (`_webdavs._tcp`). Ubuntu's Files app (GVfs) mounts it natively, so nothing extra is installed on the PC.
  - **Whole-phone mode:** `/storage/emulated/0` plus the SD card. Needs `MANAGE_EXTERNAL_STORAGE`, so the app is sideloaded rather than on Play. Root-only system folders are out of scope.
  - **Shared-folder mode:** a single folder picked with the Storage Access Framework.
- **Controller UI:** touchpad, keyboard, power, media, custom buttons, device dashboard.
- **Android integrations:** share-sheet target, quick-settings tile, home-screen widget, notification listener.

### PC (`turad`)
- **Mouse and keyboard:** injected through `uinput`, so it works on Wayland and on the login screen. Needs a udev rule plus a group.
- **Power:** shutdown, reboot, suspend and lock through the logind D-Bus API, allowed by a polkit rule. Suspend is opt-in (`turad --allow-suspend`) because some GPUs never resume from S3 (seen with an AMD RX 6700 XT). "Screen off" (Mutter DPMS) is the safe default instead.
- **Wake on LAN:** the phone sends the magic packet; the PC tells the phone its MAC address during pairing. The PC needs wired Ethernet, WoL on in the BIOS, and `nmcli ... 802-3-ethernet.wake-on-lan magic`.
- **Discovery:** turad announces `_tunnelrat._tcp` through Avahi (TXT `fp=` certificate fingerprint); the app looks for its paired PC while on Wi-Fi and not connected, and moves the found address to the front (done). A joined PC (`tura join`) looks the other PC up by name the same way when none of its addresses answer (done).
- **Settings:** one small GTK4/libadwaita settings window. Otherwise the PC side stays out of sight.

### Security
- Pairing: the PC shows a QR code containing its address, certificate fingerprint and a one-time token. The phone scans it, and both sides pin each other's certificates.
- All traffic uses mutual TLS: the control channel (WebSocket) and WebDAV alike.
- Only paired devices are accepted. Pairings can be revoked from either side.

## Features
1. Clipboard sync in both directions.
2. "Send to PC" in the Android share sheet: links open in the browser, files go to `~/Downloads`.
3. Home-screen widget and quick-settings tile for Wake and Shutdown.
4. Media remote over MPRIS: play/pause, volume, now playing with artwork.
5. Phone notifications mirrored to the PC, with reply.
6. Custom buttons that run user-defined commands on the PC.
7. Touchpad gestures: two-finger scroll, pinch, three-finger switching, presentation mode.
8. Phone battery and signal shown on the PC.

Later, not committed: phone as webcam. (Access over the Ratlines mesh is handled outside Tunnelrat.)

## Roadmap
1. **MVP:** pairing (QR + mTLS), touchpad and keyboard, Wake on LAN, shutdown/reboot/suspend/lock.
2. **Phone as a drive:** WebDAV with whole-phone and shared-folder modes, mDNS.
3. **Glue:** clipboard sync, share sheet, widget and quick-settings tile.
4. **Media remote, notifications, custom buttons, battery/signal.**
5. **Gestures polish and presentation mode.** Three-finger swipes (overview, workspaces) and three-finger tap (middle click) are done as key shortcuts. Pinch-to-zoom was dropped: Ctrl+wheel steps felt clumsy, and a virtual uinput touchpad for native pinches only worked intermittently (libinput often didn't start a pinch).
6. **Screens:**
   - PC → PC (done): `tura screen <pc>`. The viewed PC casts with Mutter's screen-cast API (pointer embedded, like PC → phone) over the link; the viewer is `tura-viewer --mode pc` (absolute pointer, buttons, wheel, evdev keys). Replaced the first version (GNOME Remote Desktop + Remmina), whose unscaled HiDPI pointer was oversized.
   - PC → phone (done): "PC screen" in the app. Mutter RemoteDesktop+ScreenCast session (portal fallback, restore token in ~/.config/tunnelrat/cast-token), pointer embedded in the picture, gst-launch with VA-API H.264 (x264 fallback), access units as binary frames, skipping to the next keyframe when the network falls behind; hardware MediaCodec decode on the phone. Trackpad mode (default) works. Direct-touch mode is broken: GNOME rejects NotifyPointerMotionAbsolute with "Invalid position" (not yet understood).
   - Phone → PC (done): "Show phone on PC" in the app. MediaProjection → MediaCodec H.264 (limited-range BT.709 signalled), binary VIDEO frames, `tura-viewer` (Python GTK4 + GStreamer, software decode: vah264dec needs h264parse) on the PC. Control through the "Tunnelrat: control from PC" accessibility service, only while mirroring. Follows rotation (re-encodes at the new size; the viewer takes the size from the stream) and drags are live (continued accessibility strokes).
   - No prompts (done): PC → phone uses Mutter's screen-cast API directly (portal only as fallback); phone → PC can be started from the PC (`tura mirror`); a PC at the login screen can be logged into from the phone.
7. **PC app** (done): `pc/app/tunnelrat-app` (GTK 4 + libadwaita, Python) with Devices (pairing QR, screens, power, drive, unpair), a drag-to-place shared-mouse layout editor, Settings (clipboard, shared input, sleep, top-bar chips) and a Status checklist; top-bar chip menu with quick actions. turad gained settings.json, GetSettings/SetSettings/GetLayouts/GetStatus, device kinds and the pairing QR.
8. **Automatic setup** (done: `pc/setup.sh`, `pc/bundle.sh` for PCs without Rust, `tura status` at the end), replacing the manual commands in the README:
   - Install packages: Deskflow, GStreamer (pipewire, x264/VA-API, libav), python3-gi + GTK 4 + libadwaita + gtk4paintablesink.
   - Install turad, tura, tura-viewer and the PC app (`pc/app/install.sh`); the systemd user unit; enable lingering (`loginctl enable-linger`) so turad runs from boot.
   - GNOME extension: copy and enable.
   - Polkit rule for power actions (tunnelrat group).
   - uinput access at the login screen: `groupadd -f uinput`, `usermod -aG uinput $USER`, the 60-tunnelrat-uinput.rules udev rule, `udevadm control --reload && udevadm trigger --name-match=uinput`.
   - Wake on LAN on the wired connection: `nmcli connection modify <wired> 802-3-ethernet.wake-on-lan magic` (NetworkManager's "default" can leave the card unable to wake the PC, seen with r8169), plus `ethtool -s <iface> wol g` for the running boot. BIOS: Wake on LAN / PCIe power-on enabled.
   - Phone (over USB, once): `adb shell appops set dev.tunnelrat PROJECT_MEDIA allow` so mirroring starts without Android's prompt; point the user to the "Tunnelrat: control from PC" accessibility setting.

## Open questions
- Is the PC that should be woken on wired Ethernet?

## Toolchain status (2026-11-10)
- Rust: not installed (needs rustup).
- Android SDK / Android Studio / adb: not installed.
- JDK: OpenJDK 25 present. The Android Gradle Plugin may want JDK 17/21; Android Studio ships its own.

## F-Droid (prepared 2026-11-12)
- Done: store texts, icon, 4 phone screenshots, changelog, v0.2.0 tag (fastlane/metadata/android/en-US), draft recipe (fdroid/dev.tunnelrat.yml), no non-free dependencies, no Google dependency metadata in the APK.
- Missing: a public repository (blocker), a test build with fdroidserver, the merge request to fdroiddata. Risk: very new build tools (Gradle 9.6, AGP 9.4, compileSdk 37) may not be supported by F-Droid's build server yet.
