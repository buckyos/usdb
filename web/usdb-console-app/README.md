# USDB Console App

This directory is the React/Vite runtime source for the control console.

The served runtime entry is the built `dist/` output from this app.

## Development

Start the control-plane first, for example:

```bash
cd /home/bucky/work/usdb
docker/scripts/run_console_preview.sh up
```

Then run the React app:

```bash
cd /home/bucky/work/usdb/web/usdb-console-app
npm install
npm run dev
```

Default local URLs:

- React dev server: `http://127.0.0.1:5174/`
- proxied control-plane target: `http://127.0.0.1:28140/`

The console also exposes an `Apps` page at `/#/apps`. It reads app entries from
`/api/system/overview` and links to the runtime-specific Balance History
Explorer, USDB Indexer Browser, and SourceDAO Web target.

When the two static explorers are opened through the control plane, their app
links use the same-origin RPC proxies:

- `/api/services/balance-history/rpc`
- `/api/services/usdb-indexer/rpc`

## Production / Docker Runtime

`usdb-control-plane` serves the built assets from:

- `web/usdb-console-app/dist`

The legacy static console remains in the repo as reference only. Whenever the
runtime entry needs to be updated, rebuild this app before rebuilding the
Docker image.

To use a different control-plane endpoint:

```bash
USDB_CONTROL_PLANE_TARGET=http://127.0.0.1:28140 npm run dev
```

To point the SourceDAO Web app card at another browser-facing target, set this
before starting `usdb-control-plane`:

```bash
CONTROL_PLANE_SOURCEDAO_WEB_URL=http://127.0.0.1:3050
```

For local full-sim, SourceDAO Web can be started from the USDB repo after
SourceDAO bootstrap completes:

```bash
docker/scripts/tools/run_local_sourcedao_web.sh up
```
