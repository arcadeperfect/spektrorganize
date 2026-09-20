#!/usr/bin/env bash
# Copy the parts of spektrafilm-rs `data/` that the runtime reads into vendor/.
# Source: the local fork next to this repo (see the path deps in Cargo.toml),
# or $SPEKTRAFILM_RS.
set -euo pipefail
cd "$(dirname "$0")/.."
SRC="${SPEKTRAFILM_RS:-../spektrafilm-rs}"
if [ ! -d "$SRC/data/profiles" ]; then echo "no spektrafilm-rs checkout at $SRC" >&2; exit 1; fi
DST=vendor/spektrafilm-data
rm -rf "$DST"
mkdir -p "$DST/profiles" "$DST/luts/spectral_upsampling" "$DST/filters" "$DST/presets"
cp "$SRC"/data/profiles/*.json "$DST/profiles/"
cp "$SRC"/data/luts/spectral_upsampling/irradiance_xy_tc.npy "$DST/luts/spectral_upsampling/"
# Reflectance upsamplers (arctic2026beta04, jakob2019, …) + their descriptors.
cp "$SRC"/data/luts/spectral_upsampling/*.toml "$DST/luts/spectral_upsampling/"
cp "$SRC"/data/luts/spectral_upsampling/*_reflectance_xy_tc.npy "$DST/luts/spectral_upsampling/"
cp "$SRC"/data/filters/neutral_print_filters.json "$DST/filters/"
cp "$SRC"/data/presets/*.toml "$DST/presets/"
echo "spektrafilm-rs $(git -C "$SRC" rev-parse --short HEAD 2>/dev/null || echo unknown) $(git -C "$SRC" branch --show-current 2>/dev/null)" > "$DST/SOURCE"
echo "synced from $SRC -> $DST"
du -sh "$DST"
