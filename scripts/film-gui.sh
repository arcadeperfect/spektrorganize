#!/usr/bin/env bash
# Launch the spektrafilm-rs GUI (with the preset Load/Save patch) for tuning presets.
# Usage: scripts/film-gui.sh [image.RAF]
set -euo pipefail
REPO="${SPEKTRAFILM_RS:-$HOME/src/devl/spektrafilm-rs}"
BIN="$REPO/target/release/spektrafilm-gui"
if [ ! -x "$BIN" ]; then
  echo "GUI not built; run: (cd $REPO && cargo build --release -p spektrafilm-gui)" >&2
  exit 1
fi
export SPEKTRAFILM_DATA_DIR="${SPEKTRAFILM_DATA_DIR:-$REPO/data}"
exec "$BIN" "$@"
