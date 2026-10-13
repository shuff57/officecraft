#!/usr/bin/env bash
# Build the browser version and deploy it to Cloudflare Pages.
#   ./scripts/deploy-web.sh               build, stage and deploy to the "officecraft" project
#   ./scripts/deploy-web.sh --build-only  build and stage the upload folder, don't deploy
#   ./scripts/deploy-web.sh --no-build    stage and deploy the existing dist/web
#   CF_PAGES_PROJECT=name ./scripts/deploy-web.sh   deploy to a different Pages project
#
# Deploying needs Cloudflare credentials: either `wrangler login` once, or CLOUDFLARE_API_TOKEN
# (Pages: Edit) and CLOUDFLARE_ACCOUNT_ID in the environment. Nothing is stored by this script.
# The staged folder (dist/pages) can also be dragged into the dashboard's Direct Upload page.
set -euo pipefail
cd "$(dirname "$0")/.."

project="${CF_PAGES_PROJECT:-officecraft}"
stage="dist/pages"
build=1
deploy=1
for arg in "$@"; do
  case "$arg" in
    --build-only) deploy=0 ;;
    --no-build) build=0 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [ "$build" = 1 ]; then
  # The Craft apps bundle their UI fonts from a directory; honour an existing setting.
  export CRAFT_FONTS_DIR="${CRAFT_FONTS_DIR:-$HOME/craft-fonts}"
  trunk build --release
fi

# Pages caps a file at 25 MiB; the wasm bundle is split into parts the page reassembles.
python3 scripts/stage-pages.py dist/web "$stage"

if [ "$deploy" = 0 ]; then
  echo "Staged $stage (not deployed)."
  exit 0
fi

if command -v wrangler >/dev/null; then
  wrangler=(wrangler)
else
  wrangler=(npx --yes wrangler)
fi
"${wrangler[@]}" pages deploy "$stage" --project-name "$project" --branch main --commit-dirty=true
