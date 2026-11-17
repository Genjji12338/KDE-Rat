#!/bin/sh
# Installs the Tunnelrat PC app for this user: the program, its app-grid
# entry and its icon. (Part of the later automatic setup.)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor"
mkdir -p "$bin" "$apps"
install -m 755 "$here/tunnelrat-app" "$bin/tunnelrat-app"
sed "s|^Exec=.*|Exec=$bin/tunnelrat-app|" "$here/dev.tunnelrat.Tunnelrat.desktop" > "$apps/dev.tunnelrat.Tunnelrat.desktop"
icon="$here/icon.png"
[ -f "$icon" ] || icon="$here/../../art/app-icon.png"
for size in 64 128 256; do
    mkdir -p "$icons/${size}x${size}/apps"
    python3 -c "
import gi, sys
gi.require_version('GdkPixbuf', '2.0')
from gi.repository import GdkPixbuf
GdkPixbuf.Pixbuf.new_from_file_at_size(sys.argv[1], int(sys.argv[2]), int(sys.argv[2])).savev(sys.argv[3], 'png', [], [])
" "$icon" "$size" "$icons/${size}x${size}/apps/dev.tunnelrat.Tunnelrat.png"
done
gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
# The app's fonts (Inter, Space Grotesk; SIL OFL, see fonts/).
fonts="$HOME/.local/share/fonts/tunnelrat"
mkdir -p "$fonts"
cp "$here"/fonts/*.ttf "$here"/fonts/OFL-*.txt "$fonts/"
fc-cache -f "$fonts" >/dev/null 2>&1 || true
update-desktop-database -q "$apps" 2>/dev/null || true
echo "Installed: Tunnelrat is in the app grid."
