#!/usr/bin/env bash
# Build OfficeCraft and install it for the current user (no root needed).
#   ./install.sh                  build from the org repos
#   ./install.sh --local          build against sibling checkouts (../wordcraft ...)
#   PREFIX=/usr/local ./install.sh   install somewhere else (may need sudo)
set -euo pipefail
cd "$(dirname "$0")"
prefix="${PREFIX:-$HOME/.local}"

if [ "${1:-}" = "--local" ]; then
  ./scripts/use-local-siblings.sh
  # Local checkouts rarely match the committed lock's pinned revisions; re-resolve for this
  # build and put the committed lock back afterwards.
  cp Cargo.lock "${TMPDIR:-/tmp}/officecraft-Cargo.lock.bak"
  trap 'cp "${TMPDIR:-/tmp}/officecraft-Cargo.lock.bak" Cargo.lock' EXIT
  cargo update -q
fi

# Extra cargo config (the updater's [patch] file that points apps at the repos you follow).
cargo_args=()
if [ -n "${OFFICECRAFT_CARGO_CONFIG:-}" ]; then
  cargo_args=(--config "$OFFICECRAFT_CARGO_CONFIG")
  # Build the committed Cargo.lock as it is: it pins each app's commit, and the browser build uses
  # the same pins. (Re-resolving here floated every app to its branch tip, so a desktop install
  # could differ from the web page.) Bump the pins with a lock commit, not on the user's machine.
  [ "${1:-}" = "--local" ] || {
    cp Cargo.lock "${TMPDIR:-/tmp}/officecraft-Cargo.lock.bak"
    trap 'cp "${TMPDIR:-/tmp}/officecraft-Cargo.lock.bak" Cargo.lock' EXIT
  }
fi

# The Craft apps bundle their UI fonts from a directory; honour an existing setting.
export CRAFT_FONTS_DIR="${CRAFT_FONTS_DIR:-$HOME/craft-fonts}"
cargo build --release "${cargo_args[@]}"

install -Dm755 target/release/officecraft "$prefix/bin/officecraft"
install -Dm644 packaging/ai.storyteller.officecraft.desktop \
  "$prefix/share/applications/ai.storyteller.officecraft.desktop"
for size in 64 128 256 512; do
  install -Dm644 "assets/app-icon/hicolor/${size}x${size}/apps/ai.storyteller.officecraft.png" \
    "$prefix/share/icons/hicolor/${size}x${size}/apps/ai.storyteller.officecraft.png"
done
install -Dm644 "assets/app-icon/hicolor/scalable/apps/ai.storyteller.officecraft.svg" \
  "$prefix/share/icons/hicolor/scalable/apps/ai.storyteller.officecraft.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/officecraft"
printf 'SOURCE_DIR=%q\nPREFIX=%q\n' "$PWD" "$prefix" > "${XDG_DATA_HOME:-$HOME/.local/share}/officecraft/install.env"
echo "Installed to $prefix (binary: $prefix/bin/officecraft)"
