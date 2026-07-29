#!/usr/bin/env bash
# Render the UI icon set (data/icons/ui/*.svg, white #F7F8F8 glyphs)
# into PNGs for the GUI:
#   ui/rendered/<name>-dark-<size>.png   — white glyph, for dark themes
#   ui/rendered/<name>-light-<size>.png  — #1E1E1E glyph, for light themes
# Sizes: 24 (mdpi) and 48 (hidpi/scale-2).
set -euo pipefail
cd "$(dirname "$0")"

mkdir -p ui/rendered

for svg in ui/*.svg; do
    name=$(basename "$svg" .svg)
    light_svg=$(mktemp --suffix=.svg)
    sed 's/#F7F8F8/#1E1E1E/g' "$svg" > "$light_svg"
    for size in 24 48; do
        rsvg-convert -w "$size" -h "$size" "$svg" \
            -o "ui/rendered/${name}-dark-${size}.png"
        rsvg-convert -w "$size" -h "$size" "$light_svg" \
            -o "ui/rendered/${name}-light-${size}.png"
    done
    rm -f "$light_svg"
done

echo "Rendered $(ls ui/*.svg | wc -l) icons x 2 variants x 2 sizes into ui/rendered/"
