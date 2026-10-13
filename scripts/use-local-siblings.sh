#!/usr/bin/env bash
# Build against sibling checkouts (../wordcraft, ../gridcraft, ../deckcraft, ../pdfcraft)
# instead of the org repos. Writes a git-ignored .cargo/config.toml.
set -euo pipefail
cd "$(dirname "$0")/.."
out=.cargo/config.toml
mkdir -p .cargo
: > "$out"
for repo in wordcraft gridcraft deckcraft pdfcraft; do
  [ -d "../$repo/crates" ] || { echo "missing ../$repo" >&2; exit 1; }
  echo "[patch.\"https://github.com/storytold/$repo\"]" >> "$out"
  for manifest in ../$repo/crates/*/Cargo.toml; do
    name=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$manifest" | head -1)
    echo "$name = { path = \"$(dirname "$manifest")\" }" >> "$out"
  done
  echo >> "$out"
done
# pdfcraft's vendored forks (same sources the manifest patches from git).
echo '[patch.crates-io]' >> "$out"
for v in hayro-interpret hayro hayro-syntax hayro-jbig2 lopdf winit egui-wgpu; do
  echo "$v = { path = \"../pdfcraft/vendor/$v\" }" >> "$out"
done
echo "wrote $out"
