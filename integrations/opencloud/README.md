# OfficeCraft for OpenCloud

An OpenCloud Web extension that opens `.docx .docm .dotx .odt .rtf .xlsx .xlsm .pptx .potx .ppsx .pdf`
in OfficeCraft's browser build, in place of a WOPI office server (Euro-Office / OnlyOffice /
Collabora). It also adds "OfficeCraft document / spreadsheet / presentation" to the **New** menu.

## How it works

The extension's editor (`src/App.vue`) frames `/craft/officecraft/?host=parent` and talks to it
through the bridge in `../../src/host.rs`:

| Message | Direction | Meaning |
|---|---|---|
| `officecraft:ready` | app → page | The suite is running; the page sends the file. |
| `officecraft:open { name, bytes }` | page → app | Open this file (empty bytes = a blank new file). |
| `officecraft:save { name, saveAs, bytes }` | app → page | Save. Same name → saved over the opened file through OpenCloud's AppWrapper (ETag conflict checks). Another name (Export as PDF, Save As) → written next to it in the same folder. |
| `officecraft:dirty { dirty }` | app → page | Unsaved work; the page guards leaving. |
| `officecraft:failed { error }` | app → page | No WebGPU/WebGL2 etc. |

The bridge only answers a parent on the **same origin**, so the OfficeCraft build must be served
from the OpenCloud host itself (on the NAS, Caddy serves `/opt/stacks/caddy/data/craft` at
`https://drive.lefthanddev.com/craft/`).

## Build and deploy

The NAS runs OpenCloud **4.0.8**, whose web UI still loads extensions as **AMD** modules through
`require.js`. Current web-extensions `main` builds ESM / Module Federation bundles that only load on
OpenCloud 6+ ("cannot create application, Cannot read properties of undefined (reading 'appInfo')").
So build from the last AMD commit of
[opencloud-eu/web-extensions](https://github.com/opencloud-eu/web-extensions), `7e795507`
(extension-sdk 6, pnpm 10.33.0):

```sh
git clone https://github.com/opencloud-eu/web-extensions ~/src/web-extensions
git -C ~/src/web-extensions worktree add ~/src/web-extensions-amd 7e795507
cp -r integrations/opencloud ~/src/web-extensions-amd/packages/web-app-officecraft
cd ~/src/web-extensions-amd && npx pnpm@10.33.0 install
cd packages/web-app-officecraft && npx pnpm@10.33.0 build      # dist/js/officecraft-*.js starts with define(
rsync -a --delete dist/ minisforum:/media/tank/AppData/opencloud/data/web/assets/apps/officecraft/
ssh minisforum 'cd /opt/stacks/opencloud && docker compose restart'
```

After an OpenCloud upgrade to 6 or later, build from web-extensions `main` instead (bump the
`^6.0.0` deps in package.json to the matching major).

and the OfficeCraft web build itself:

```sh
CRAFT_FONTS_DIR=~/craft-fonts trunk build --release
rsync -a --delete dist/web/ minisforum:/opt/stacks/caddy/data/craft/officecraft/
```

`src/manifest.json`'s `config.url` (default `/craft/officecraft/`) can be overridden per server in
OpenCloud's `apps.yaml` under `officecraft: config: url:`.
