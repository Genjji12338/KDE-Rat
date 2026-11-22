# KDE Rat

KDE Rat adapts the PC side of [Tunnelrat](https://github.com/I232826/tunnelrat) for Debian and KDE Plasma. This repository starts from Tunnelrat **v0.4.0** and keeps the upstream Android app and shared protocol source alongside the KDE-focused PC changes.

## Install on Debian with KDE Plasma

```sh
git clone https://github.com/Genjji12338/KDE-Rat.git
cd KDE-Rat/pc
./setup.sh
```

The installer builds the PC tools from source, installs the GTK desktop app and viewer, enables the user service, and configures Plasma login autostart for the desktop session. It can install system packages and permissions with `sudo`; see [the Debian/KDE guide](README-DEBIAN-KDE.md) before running it.

## Upstream and contributors

KDE Rat is a downstream adaptation of Tunnelrat by [I232826](https://github.com/I232826). See [CONTRIBUTORS.md](CONTRIBUTORS.md) for attribution and the scope of the KDE Rat changes.

## License

KDE Rat is licensed under **GNU GPL v3.0 only**. See [LICENSE](LICENSE). Fonts retain their separate SIL Open Font License notices.
