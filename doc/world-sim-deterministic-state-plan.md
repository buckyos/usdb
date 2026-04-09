# World-Sim Deterministic State Plan

## 1. Goal

This document defines how the Docker world-sim should handle:

- continuous runtime
- persistent state across restarts
- explicit reset behavior
- seed-driven simulation behavior

The immediate goal is not to make the whole simulated world derivable from one
seed. The first goal is to make runtime behavior explicit and operationally
safe.

## 2. Current Runtime Model

The Docker world-sim is now split into two phases:

1. `world-sim-bootstrap`
2. `world-sim-runner`

### 2.1 `world-sim-bootstrap`

This one-shot phase is responsible for:

- waiting for `btc-node`, `ord-server`, `balance-history`, and `usdb-indexer`
- ensuring the miner wallet exists
- premining up to the configured bootstrap height
- creating or loading agent wallets
- allocating agent receive addresses
- funding agent wallets
- waiting until BTC, `balance-history`, and `usdb-indexer` all observe the same
  height
- writing a bootstrap marker into the shared `world-sim-data` volume

The bootstrap phase is intended to run exactly once for one persisted world
state.

### 2.2 `world-sim-runner`

This phase is responsible for:

- verifying the bootstrap marker
- loading the persisted bootstrap state
- running the simulator against the current BTC / ord / USDB state

This phase no longer performs funding or wallet bootstrap as part of normal
steady-state simulation.

## 3. Persistence Model

World-sim state is currently persisted by Docker volumes, mainly:

- `btc-data`
- `ord-data`
- `balance-history-data`
- `usdb-indexer-data`
- `world-sim-data`

This means:

- `docker/scripts/run_world_sim.sh down`
  - stops containers
  - keeps world state
- `docker/scripts/run_world_sim.sh up`
  - resumes from the same underlying BTC / ord / USDB state
- `docker/scripts/run_world_sim.sh reset`
  - removes volumes
  - starts from a clean world next time

The operator helper now also exposes a startup state policy:

- `WORLD_SIM_STATE_MODE=persistent`
- `WORLD_SIM_STATE_MODE=reset`
- `WORLD_SIM_STATE_MODE=seeded-reset`

The bootstrap marker and loop state are stored under `world-sim-data`, so the
runner can distinguish between:

- an initialized world
- an uninitialized world

## 4. Continuous Runtime

The simulator now supports two modes.

### 4.1 Bounded Mode

When:

- `SIM_BLOCKS > 0`

the runner executes one bounded batch and exits successfully.

This is useful for:

- smoke tests
- deterministic short runs
- CI-style validation

### 4.2 Continuous Mode

When:

- `SIM_BLOCKS = 0`

the runner stays alive and repeatedly executes bounded batches of:

- `SIM_LOOP_BATCH_BLOCKS`

This keeps the simulated network active until it is stopped manually.

The loop state records:

- completed batch count
- last batch seed
- last observed block height

This is enough to make the runtime mode operationally stable, even though it is
not yet sufficient for full deterministic replay after arbitrary crashes.

## 5. Seed Semantics Today

The current implementation uses:

- `SIM_SEED`

to control simulator randomness, such as:

- action selection
- diagnostic sampling

This seed does **not** fully define:

- BTC wallet private keys
- ord wallet identities
- agent receive addresses
- current UTXO set
- current inscription ownership
- current pass / energy / protocol state

Those currently come from the persisted Docker volumes.

So the actual model today is:

- `SIM_SEED` controls **how agents behave**
- Docker volumes control **what world they are acting in**

## 6. What Is Deterministic Today

The following is deterministic today:

- given the same persisted world state
- given the same `SIM_SEED`
- given the same batch index

the simulator will choose the same random decisions inside that batch.

This is already useful for:

- repeated local debugging
- comparing protocol behavior across code changes

## 7. What Is Not Deterministic Yet

The following is not fully deterministic yet:

- recovering mid-batch after a crash and reproducing the exact same next action
- deriving the full BTC / ord / USDB world from one single seed

That requires additional work.

## 8. Exact Mid-Batch Replay and Crash Recovery

For world-sim, exact mid-batch replay means:

- a bounded batch starts from a known BTC / ord / USDB state
- each tick inside that batch has a stable action plan
- if the runner dies before that batch finishes, the next run can regenerate the
  same planned actions for the unfinished portion of the batch

Today the operator can rerun a failed batch from the same batch seed, but the
simulator still depends on process-local RNG progression. That means it does
not yet have a stable notion of:

- action slot identity inside a tick
- action selection at a specific position
- action-specific random choices such as prev selection, transfer target
  selection, or send/spend amount

To support exact replay later, the simulator needs two layers:

1. deterministic action planning
2. persistent recovery checkpoints

The first layer makes the action sequence reproducible. The second layer makes
it possible to resume from the middle of an unfinished batch after a crash.

## 9. Next-Stage Deterministic Design

The recommended next-stage model is:

### 8.1 Separate Identity and Action Seeds

- `WORLD_SIM_IDENTITY_SEED`
  - deterministically derives miner / agent identities
- `SIM_SEED`
  - determines action policy

### 8.2 Explicit State Mode

- `persistent`
  - keep using volumes
- `reset`
  - start from empty state
- `seeded-reset`
  - start from empty state and deterministically recreate wallets, addresses,
    and bootstrap allocation

The current implementation has now started exposing this contract at the
operator layer:

- `persistent`
  - preserves the current world and reuses the existing bootstrap marker
- `reset`
  - clears Docker volumes before startup
- `seeded-reset`
  - clears Docker volumes before startup
  - requires `WORLD_SIM_IDENTITY_SEED`
  - deterministically recreates miner and agent ord wallet identities from the
    chosen seed
  - records the chosen identity seed and identity scheme in the bootstrap
    marker

This is still an intermediate step. It does **not** yet make the entire BTC /
ord / USDB world derivable from one seed, and it does not yet support exact
mid-batch replay after arbitrary crashes.

### 9.3 Position-Derived Action Planning

Instead of only using process-local RNG state, future batches should derive
randomness from stable coordinates such as:

- base seed
- batch seed
- tick
- action slot
- action phase

This gives each action position a stable deterministic identity.

The immediate form of this design is:

- derive a dedicated RNG for each `tick / slot / phase`
- compute a stable `action_id`
- use that position-derived RNG for:
  - action slot count
  - actor selection
  - action selection
  - action-specific random choices

Once this exists, later recovery logic can refer to a stable `action_id` rather
than "whatever the next `random.Random()` call would have produced".

### 9.4 Recovery Checkpoints

After deterministic action planning is in place, the next layer is to persist a
recovery cursor, for example:

- batch seed
- last completed tick
- last completed action slot inside the current tick
- action ids that have already been fully applied

This is the layer that enables actual crash recovery rather than just replaying
the whole batch from the beginning.

## 10. First-Batch Implementation Boundary

This batch intentionally implements only:

- bootstrap/loop split
- explicit persistent vs reset operator behavior
- continuous mode with batch looping
- loop state persistence
- deterministic identity recreation for `seeded-reset`
- stable action planning primitives:
  - position-derived RNG
  - stable action ids
  - action-specific randomness derived from action position

It does **not** yet implement:

- full seeded-reset replay of the entire protocol state
- persistent mid-batch checkpoints
- exact mid-batch crash replay
- deterministic reconstruction of the complete BTC / ord / USDB world from one
  seed alone

## 11. Runtime Stability Gates

To reduce the ord wallet / ord server race observed immediately after funding
and bootstrap, the runtime now performs an explicit stability gate before the
simulation loop starts:

1. wait until `ord-server` reaches the current BTC height
2. wait until `balance-history` and `usdb-indexer` reach the same height and
   report `consensus_ready=true`
3. probe each agent ord wallet with repeated `ord wallet balance` calls

These probes are controlled by:

- `WORLD_SIM_ORD_STABILITY_PROBES`
- `WORLD_SIM_ORD_STABILITY_SLEEP_SECS`

## 12. Operator Guidance

Recommended operator meanings:

- `down`
  - keep state
- `reset`
  - destroy state
- `SIM_BLOCKS > 0`
  - bounded validation run
- `SIM_BLOCKS = 0`
  - continuous simulated network

This is the minimum model needed to support:

- live local demos
- wallet integration later
- a stable local testnet-like BTC world for console and protocol testing
