#!/usr/bin/env bash
# Dev helper: install the Travel Mode icon into the user's hicolor icon
# theme so window/taskbar icons resolve by name
# (com.github.missingfoot.travelmode).
set -euo pipefail
cd "$(dirname "$0")/.."

ICON_NAME="com.github.missingfoot.travelmode"
HICOLOR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"

# Scalable icon. The light variant (dark #1E1E1E glyph) is installed
# because it stays visible on light taskbars/panels; if you use a dark
# panel, swap in travelmode-dark.svg instead.
install -Dm644 data/icons/travelmode-light.svg \
    "$HICOLOR/scalable/apps/$ICON_NAME.svg"

# Raster fallbacks (dark variant = white glyph).
for size in 128 256; do
    install -Dm644 "data/icons/travelmode-dark-$size.png" \
        "$HICOLOR/${size}x${size}/apps/$ICON_NAME.png"
done

# Refresh the cache if a tool is available; absence is fine.
if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
    gtk4-update-icon-cache -q "$HICOLOR" || true
elif command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q "$HICOLOR" || true
fi

echo "Installed $ICON_NAME icon into $HICOLOR"
