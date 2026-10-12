#!/usr/bin/env bash
# Update OfficeCraft: rebuild it against the newest commits of the app repos you follow.
#
#   update.sh check   one line per component: name<TAB>state<TAB>where<TAB>revision
#                     state is current, update, new (never installed) or unreachable
#   update.sh apply   fetch, rebuild, reinstall; streams build output
#
# Which repo and branch each app follows lives in ~/.config/officecraft/sources, one app per
# line:  <app> <git url> [branch]   (default branch: main). Missing apps use the org repo.
# Example:
#   wordcraft https://github.com/shuff57/wordcraft equashion-editor
#   deckcraft https://github.com/shuff57/deckcraft equashion-editor
#   gridcraft file:///home/me/src/gridcraft main
set -euo pipefail

cfg_home="${XDG_CONFIG_HOME:-$HOME/.config}/officecraft"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}/officecraft"
sources_file="$cfg_home/sources"
installed_file="$data_home/installed"
env_file="$data_home/install.env"
org="https://github.com/storytold"
apps=(wordcraft gridcraft deckcraft pdfcraft)

[ -f "$env_file" ] || { echo "No install record at $env_file: run ./install.sh from a checkout first." >&2; exit 2; }
# shellcheck disable=SC1090
. "$env_file"   # SOURCE_DIR, PREFIX
git -C "${SOURCE_DIR:-/nonexistent}" rev-parse --git-dir >/dev/null 2>&1 || { echo "Source checkout ${SOURCE_DIR:-?} is gone: run ./install.sh again." >&2; exit 2; }

declare -A url branch
for a in "${apps[@]}"; do url[$a]="$org/$a"; branch[$a]=main; done
if [ -f "$sources_file" ]; then
  while read -r app u b _; do
    case "$app" in ''|'#'*) continue ;; esac
    [ -n "${url[$app]+x}" ] || { echo "sources: unknown app '$app'" >&2; exit 2; }
    [ -n "${u:-}" ] || { echo "sources: '$app' needs a git url" >&2; exit 2; }
    url[$app]="$u"; branch[$app]="${b:-main}"
  done < "$sources_file"
fi

remote_sha() { git ls-remote "$1" "refs/heads/$2" 2>/dev/null | cut -f1 | head -1; }
recorded() { [ -f "$installed_file" ] && awk -v k="$1" '$1==k {print $2}' "$installed_file" || true; }

cmd="${1:-check}"
case "$cmd" in
check)
  for a in "${apps[@]}"; do
    sha=$(remote_sha "${url[$a]}" "${branch[$a]}")
    have=$(recorded "$a")
    if [ -z "$sha" ]; then state=unreachable
    elif [ -z "$have" ]; then state=new
    elif [ "$sha" = "$have" ]; then state=current
    else state=update; fi
    printf '%s\t%s\t%s %s\t%s\n' "$a" "$state" "${url[$a]}" "${branch[$a]}" "${sha:0:8}"
  done
  # OfficeCraft itself: its own checkout against its upstream branch.
  if up=$(git -C "$SOURCE_DIR" rev-parse --abbrev-ref '@{u}' 2>/dev/null); then
    git -C "$SOURCE_DIR" fetch -q 2>/dev/null || true
    behind=$(git -C "$SOURCE_DIR" rev-list --count 'HEAD..@{u}' 2>/dev/null || echo 0)
    state=current; [ "$behind" -gt 0 ] && state=update
    printf 'officecraft\t%s\t%s\t%s behind\n' "$state" "$up" "$behind"
  fi
  ;;
apply)
  cd "$SOURCE_DIR"
  if [ -z "$(git status --porcelain --untracked-files=no)" ] && git rev-parse '@{u}' >/dev/null 2>&1; then
    echo "== OfficeCraft: pulling"
    git pull --ff-only -q || echo "   (could not fast-forward; building the checkout as it is)"
  else
    echo "== OfficeCraft: checkout has local changes or no upstream; building it as it is"
  fi

  # Crates each org repo provides, from the committed lock (the working lock may hold paths).
  lock=$(git show HEAD:Cargo.lock)
  patch_file="$data_home/patches.toml"
  mkdir -p "$data_home"
  : > "$patch_file"
  for a in "${apps[@]}"; do
    if [ "${url[$a]}" = "$org/$a" ]; then
      [ "${branch[$a]}" = main ] || echo "   note: $a follows $org/$a branch ${branch[$a]}; cargo cannot redirect a repo to itself, using its default branch" >&2
      continue
    fi
    names=$(printf '%s\n' "$lock" | awk -v a="$a" '/^name = /{n=$3; gsub(/"/,"",n)} /^source = "git\+https:\/\/github.com\/storytold\//{ if (index($0, "storytold/" a "#")||index($0, "storytold/" a "\"")) print n}' | sort -u)
    printf '[patch."%s/%s"]\n' "$org" "$a" >> "$patch_file"
    while read -r n; do
      [ -n "$n" ] && printf '%s = { git = "%s", branch = "%s" }\n' "$n" "${url[$a]}" "${branch[$a]}" >> "$patch_file"
    done <<< "$names"
    echo >> "$patch_file"
  done
  # pdfcraft's vendored forks come in through [patch.crates-io].
  if [ "${url[pdfcraft]}" != "$org/pdfcraft" ]; then
    echo '[patch.crates-io]' >> "$patch_file"
    awk '/^\[patch.crates-io\]/{on=1;next} /^\[/{on=0} on && NF {print $1}' Cargo.toml | while read -r n; do
      printf '%s = { git = "%s", branch = "%s" }\n' "$n" "${url[pdfcraft]}" "${branch[pdfcraft]}"
    done >> "$patch_file"
  fi
  echo "== Rebuilding against:"
  for a in "${apps[@]}"; do echo "   $a  ${url[$a]} ${branch[$a]}"; done

  OFFICECRAFT_CARGO_CONFIG="$patch_file" PREFIX="$PREFIX" ./install.sh

  mkdir -p "$data_home"; : > "$installed_file"
  for a in "${apps[@]}"; do
    printf '%s %s\n' "$a" "$(remote_sha "${url[$a]}" "${branch[$a]}")" >> "$installed_file"
  done
  echo "== Done. Restart OfficeCraft to use the new build."
  ;;
*) echo "usage: update.sh check|apply" >&2; exit 2 ;;
esac
