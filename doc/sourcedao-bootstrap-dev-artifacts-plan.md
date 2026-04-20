# SourceDAO Bootstrap Dev Artifacts Plan

## 1. Scope

This document defines the **development-phase** path for `sourcedao-bootstrap`.

It is intentionally narrower than the long-term job described in:

- `doc/sourcedao-bootstrap-job-plan.md`

Current stage-one scope:

- consume the local `SourceDAO` workspace from the developer machine
- consume prebuilt `artifacts-usdb`
- run a one-shot bootstrap against a live ETHW RPC
- initialize and validate the genesis-predeployed:
  - `Dao`
  - `Dividend`
- wire `Dao.setTokenDividendAddress(...)`
- write a local bootstrap state file and completion marker

Current non-goals:

- no runtime contract compilation by default
- no runtime deployment of `Committee`, `Project`, `DevToken`, `NormalToken`, `TokenLockup`, or `Acquired`
- no published artifact bundle flow yet

## 2. Why This Stage Exists

`SourceDAO` bootstrap still needs real chain-side execution, but the contract
artifacts and bootstrap smoke logic already exist in the workspace repo:

- `/home/bucky/work/SourceDAO`

The shortest path to a usable Docker one-shot job is:

1. reuse the local `SourceDAO` workspace
2. reuse `scripts/usdb_bootstrap_smoke.ts`
3. require `artifacts-usdb` to exist
4. keep the job focused on `Dao`/`Dividend`

This provides a real bootstrap step without freezing the final release artifact
format too early.

## 3. Development Inputs

Stage-one `sourcedao-bootstrap` consumes:

- local `SourceDAO` repository
- `SourceDAO` `node_modules`
- `SourceDAO` `artifacts-usdb`
- copied bootstrap config under:
  - `docker/local/bootstrap/manifests/ethw-bootstrap-config.json`
  - `docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json`

Recommended preparation:

```bash
cd /home/bucky/work/SourceDAO
npm ci
npm run build:usdb

cp tools/config/sourcedao-bootstrap-full.example.json \
  /home/bucky/work/usdb/docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json
```

Notes:

- the config file still carries `rpcUrl`, but the Docker job overrides it with
  `--rpc-url "${ETHW_RPC_URL}"`
- the Docker job also rewrites a runtime copy of the config under `/bootstrap`
  so `artifactsDir` resolves correctly inside the container
- the recommended long-term source is now:
  - `SourceDAO/tools/config/sourcedao-bootstrap-full.example.json`
- full bootstrap should prefer explicit config sections over script fallbacks:
  - `committee`
  - `devToken`
  - `normalToken`
  - `project`
  - `tokenLockup`
  - `acquired`

## 4. Docker Mode

Stage-one uses:

- `SOURCE_DAO_BOOTSTRAP_MODE=dev-workspace`

The one-shot job runs in a Node image and bind-mounts:

- the `SourceDAO` workspace
- the shared `/bootstrap` volume
- the USDB Docker helper scripts

The default mode remains:

- `SOURCE_DAO_BOOTSTRAP_MODE=disabled`

So the bootstrap overlay does not require the local `SourceDAO` workspace unless
the operator explicitly enables it.

Current scopes:

- `SOURCE_DAO_BOOTSTRAP_SCOPE=dao-dividend-only`
  - stage-one
  - only initializes the genesis-predeployed `Dao` / `Dividend`
- `SOURCE_DAO_BOOTSTRAP_SCOPE=full`
  - development-only stage-two
  - also deploys and wires:
    - `Committee`
    - `DevToken`
    - `NormalToken`
    - `Project`
    - `TokenLockup`
    - `Acquired`

## 5. Prepare Modes

Stage-one supports two prepare modes:

- `SOURCE_DAO_BOOTSTRAP_PREPARE=validate`
  - default
  - require `node_modules` and `artifacts-usdb` to already exist
  - fail fast if prerequisites are missing
- `SOURCE_DAO_BOOTSTRAP_PREPARE=auto`
  - development convenience mode
  - run `npm ci` if `node_modules` is missing
  - run `npm run build:usdb` if `artifacts-usdb` is missing

`auto` is useful during active development, but it should not become the final
release path.

## 6. Output Files

When the one-shot job succeeds it writes:

- `/bootstrap/sourcedao-bootstrap-state.json`
- `/bootstrap/sourcedao-bootstrap.done.json`

The state file is meant for inspection and control-plane display. The done file
is a completion marker.

Stage-one state should at least report:

- `mode`
- `scope`
- `status`
- `current_step`
- `rpc_url`
- `repo_dir`
- `artifacts_dir`
- `config_path`
- `dao_address`
- `dividend_address`
- `chain_id`
- `completed_at`

For `full` scope, the state file should also be updated incrementally while the
job is running so operators can see:

- which module is currently being deployed or wired
- which modules are already known on-chain
- whether a failure happened before or after a module deployment transaction

## 7. Relationship To Later Phases

This development path now has two execution layers:

- `usdb_bootstrap_smoke.ts`
  - `dao-dividend-only`
- `usdb_bootstrap_full.ts`
  - `full`

Later phases should still replace workspace-mounted inputs with a release-grade
artifact bundle.

That later work should build on the same:

- `/bootstrap/sourcedao-bootstrap-state.json`
- `/bootstrap/sourcedao-bootstrap.done.json`

so operators and the control-plane keep a stable state model.
