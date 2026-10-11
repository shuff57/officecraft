# HANDOFF: OfficeCraft (tabbed Craft-suite shell)
Written 2026-10-09. Project: `~/officecraft`. Local git repo, branch `main`, one commit `828d88a`, nothing pushed.

## What it is
One native eframe/egui window hosting WordCraft, GridCraft, DeckCraft and PdfCraft as tabs
(storytold's Rust "Craft" apps, Apache-2.0). Same idea as GenOffice's shell (`genspark-ai/genoffice`,
`apps/shell`), but the editors run in-process, not as separate windows.

Run: `~/officecraft/target/release/officecraft [files...]`. Installed copy: `~/.local/bin/officecraft`
(app-menu entry `~/.local/share/applications/officecraft.desktop`, icon `officecraft.svg`).

## Done and verified
- **Tabs:** Home + one tab per open file, many per app. Files route by extension (`kind_for` in `src/main.rs`).
  Reopening a path focuses its tab. `+` menu starts a blank tab. Close prompts when the tab has unsaved work.
- **Home:** sidebar (Recent, user-added folders), quick-start cards, filter pills, search, files table, drag-drop overlay.
- **Assistant panel** (`src/ai.rs`, toggle top-right): OpenAI-compatible chat. Tools `inspect_document`,
  `list_commands`, `run_command`, `read_range` (Sheets), mapped onto each app's control methods
  (`engine.execute`, `document.inspect`, ...; PDF uses its channel client `ui.command`). Tested end to end on
  Docs with `qwen3:0.6b`: a valid `text.insert` landed in the document; a bad command id came back as an error.
- **Font crash fix:** all apps share one `egui::Context`; each installs its own fonts/visuals. Switching apps
  panicked (`FontFamily::Name("semibold") is not bound`). Shell now calls `context_changed()` on the app being
  switched to (`ctx_owner` in `Suite::logic`).
- **Session restore:** open file paths (active last, `#home` marker) saved to `~/.config/officecraft/session.txt`,
  reopened on launch. Recents: `recent.txt`; folders: `folders.txt` (same dir).
- **Self-tests:** `OFFICECRAFT_SELFTEST=1` cycles 12 view switches, exits 0. `OFFICECRAFT_SELFTEST_AI="<prompt>"`
  runs one prompt through the Docs assistant, prints transcript + document, exits 0/1.
  Set `XDG_CONFIG_HOME` to a temp dir when testing so your real session/recents aren't touched.
- **Assistant config:** defaults to `http://127.0.0.1:11434/v1` (local Ollama), model `qwen3:0.6b`
  (small and fast on purpose, "will upgrade later"). Override with env `OFFICECRAFT_AI_URL` / `OFFICECRAFT_AI_MODEL`
  or `~/.config/officecraft/ai.json` `{base_url, model}`. Key, if any: env `OFFICECRAFT_AI_KEY`. Never written to disk.

## Not verified / known gaps
- **No visual check.** Every run was headless or log-only. Layout, Enter-to-send, scrolling, close prompt,
  folder listing and drop overlay have never been seen by eye.
- Assistant only exercised on **Docs**. Sheets, Slides and PDF (async channel path) untested.
- Per-app wiring skipped: prefs, crash recovery, control sockets; DeckCraft audio is silent.
- Unsaved new tabs keep the placeholder name ("Docs 1") after Save As.
- No tab reorder/overflow; folder view is one level deep; no streaming in the assistant; assistant history not saved.
- Tear-off/dock windows need egui secondary viewports, not attempted.
- `qwen3:0.6b` invents command ids unless told them; use `qwen3-coder:30b` for real work.

## Warnings
1. **A second Claude session is editing this same directory.** It added a Graph tab (viSHual engine,
   `src/graph.rs`, path dep on `../viSHual/crates/viz_native` (the native viSHual app), `Kind::Graph`, `ai::App::Graph`). It has
   uncommitted changes to `Cargo.toml`, `src/graph.rs`, `src/main.rs` since commit `828d88a`.
   Do not overwrite or revert them. Check `git status`/`git diff` before editing `main.rs`.
2. **Installed binary is stale.** `~/.local/bin/officecraft` predates session restore, the model default and
   the Graph tab. It was deliberately not refreshed because the current tree contains the other session's
   unfinished work. Reinstall once that session is done: `install -Dm755 target/release/officecraft ~/.local/bin/officecraft`.
   A running window keeps the old binary; closing it loses unsaved work, so ask first.
3. **Four sibling repos carry uncommitted patches** `context_changed()` (7 to 8 lines each) in
   `crates/ui-egui/src/lib.rs` of `~/wordcraft`, `~/gridcraft`, `~/deckcraft`, `~/pdfcraft`. The shell needs them;
   a `git pull`/reset there will break the build. Not saved as patch files yet.
4. **Build layout is fragile:** `Cargo.toml` uses `../wordcraft`, `../gridcraft`, `../deckcraft`, `../pdfcraft`
   path deps and repeats pdfcraft's `[patch.crates-io]` vendored forks (hayro, lopdf, winit, egui-wgpu)
   because cargo ignores `[patch]` from path dependencies. Without it pdfcraft-render fails to compile.
   `Cargo.lock` was seeded from pdfcraft's.
5. `gridcraft` and `pdfcraft` were cloned fresh for this (`--depth 1`). The ArtCraft Launcher was built from source at `~/craft-launcher` (installed to `~/.local/bin/artcraft-launcher`).

## Next, in rough order
1. Look at it: launch, click through Docs/Sheets/Slides/PDF and Home, report visual bugs.
2. Save the four `context_changed()` diffs into `patches/` so the siblings can be rebuilt.
3. Reinstall the build once the Graph tab work lands; relaunch.
4. Exercise the assistant on Sheets/Slides/PDF; try `qwen3-coder:30b`.
5. Per-file tab rename after Save As; wire prefs/recovery; then consider the assistant's history and streaming.
6. Optional milestone: tear-off windows via egui viewports (spike first).

## Cleanup candidates
- `~/genoffice-rea/` (~547 MB: extracted GenOffice rpm + source clone, used for the layout review).
- `ollama rm qwen3:0.6b` (522 MB) if the tiny model is no longer wanted.
- The REA MCP server disconnected mid-session; the binary-level analysis of GenOffice was never completed
  (the layout review used its public source instead).
