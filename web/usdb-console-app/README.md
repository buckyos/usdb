# USDB Console App

This directory is the React/Vite runtime source for the control console.

The served runtime entry is the built `dist/` output from this app.

## Development

Start the control-plane first, for example:

```bash
cd /home/bucky/work/usdb
docker/scripts/tools/run_local_console.sh up
```

Retrieve the login token in another terminal:

```bash
docker/scripts/tools/run_local_console.sh token
```

Authentication also applies to the Vite proxy. The development servers bind to
loopback and preserve Host so same-origin checks work through the proxy.
The host observer is supplied by installed node kits; a dev-sim without it shows
an explicit missing-observer state. For isolated development wallet tooling, set
`CONTROL_PLANE_DEVELOPMENT_ENABLED=true` before starting the dev-sim container;
regtest/development-chain gates still apply. Never enable it on a shared node.

Then run the React app:

```bash
cd /home/bucky/work/usdb/web/usdb-console-app
npm ci
npm run dev
```

Default local URLs:

- React dev server: `http://127.0.0.1:5174/`
- proxied control-plane target: `http://127.0.0.1:28140/`

The console also exposes an `Apps` page at `/#/apps`. It reads app entries from
`/api/system/overview` and links to the runtime-specific Balance History
Explorer, USDB Indexer Browser, and SourceDAO Web target.

## Wallet identity boundary

`/#/me/usdb` and `/#/me/btc` use `WalletIdentityPage` for read-only wallet and
watch-address lookup. `walletIdentity.ts` owns provider discovery, bounded reads,
account/network event invalidation and session disposal. No account permission
request is sent before the operator clicks Connect. EVM reads compare chain ID
and genesis with the fresh host identity and the backend RPC observation. BTC
queries compare the actual Bitcoin network, independently of the USDB network.
The node's configured miner address is a public observation, never wallet authority.

Provider contracts: [EIP-1193](https://eips.ethereum.org/EIPS/eip-1193),
[EIP-6963](https://eips.ethereum.org/EIPS/eip-6963),
[EIP-3326](https://eips.ethereum.org/EIPS/eip-3326),
[UniSat](https://docs.unisat.io/developer-support/open-api-documentation/unisat-wallet),
[OKX Bitcoin](https://web3.okx.com/zh-hans/onchainos/dev-docs/wallet/dapp-connect/chains/bitcoin/provider).
Unknown BTC chains (including Fractal) must not fall back to Bitcoin mainnet.
Missing genesis/network observations remain unverified. New adapters should add
provider-event and late-response tests before enabling transaction operations.

The legacy `MePage` is only reachable under `/#/development/usdb` or
`/#/development/btc` with the backend development flag enabled. Its development
WIF stays in memory and is cleared on leaving the page or ending the session;
legacy localStorage WIF records are discarded, never restored.

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

Operator installation, SSH access, freshness semantics and troubleshooting are
covered in [the private-console handbook](../../doc/handbook/services/control-plane.md).

The optional end-to-end regression uses an isolated Rust process and Chromium:

```bash
cargo build --locked --manifest-path src/btc/Cargo.toml -p usdb-control-plane
# Build all three web apps, then use a Python environment with Playwright/Chromium.
python3 tests/test_control_plane_browser.py
python3 tests/test_control_plane_wallet_browser.py
python3 tests/test_node_notifications_browser.py
node tests/test_control_plane_wallet.mjs
```

Run these commands from the repository root. Test RPC endpoints are deliberately
unavailable, and no existing node or wallet is used. The wallet browser test
provides fake extensions and controlled query responses; it does not qualify a
real extension release. `npm run test:wallet` runs the provider/session regression
from this app directory and is also part of the services image build.
