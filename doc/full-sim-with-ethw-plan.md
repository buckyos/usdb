# Full-Sim With ETHW Plan

## 1. Goal

This document defines the first staged integration of `ethw-node` into the
Docker `dev-sim + world-sim` environment.

The immediate goal is not to add a full ETHW-side simulator. The first goal is
smaller and more useful:

- keep BTC-side `world-sim` unchanged as the main source of protocol activity
- allow `run_local_world_sim_ethw.sh up` to start a real `ethw-node`
- give that ETHW node a deterministic miner identity
- make the ETHW miner address align with the future miner-pass `eth_main`
  binding model

## 2. Two Different Identity Layers

ETHW integration has two separate identity concepts:

1. **Chain miner identity**
   - the ETH address used by `ethw-node` as its mining reward address
   - configured through `--miner.etherbase` / `--etherbase`

2. **Protocol miner identity**
   - the `eth_main` field recorded in BTC miner-pass inscriptions
   - defined by the miner-certificate inscription protocol

These are different concepts, but for a protocol-consistent full-sim they
should eventually converge to the same ETH address.

## 3. Current Constraint

Today `run_local_world_sim_ethw.sh up` only keeps the normal `ethw-node` in the
graph. It does not yet bind ETHW mining identity to the world-sim seed or to
the BTC miner-pass model.

This means:

- ETHW can run as a peer service
- ETHW can be observed by the control console
- but ETHW mining identity is not yet deterministic or protocol-aligned

## 4. First Batch Scope

The first batch intentionally stays small:

1. introduce a deterministic ETHW miner identity for `run_local_world_sim_ethw.sh up`
2. derive that identity from `WORLD_SIM_IDENTITY_SEED` unless explicitly
   overridden
3. persist a small local identity marker under the ETHW data directory
4. export `ETHW_MINER_ADDRESS` into the runtime shell used to launch `geth`
5. keep bootstrap/canonical-genesis flows unchanged

This first batch does **not** yet:

- make BTC miner-pass mint automatically use the same ETH address
- add ETHW-side simulated transactions or contract actions
- add multi-node ETHW simulation
- replace the dedicated bootstrap overlay

For full-sim runtime stability, the recommended BTC RPC mode is also:

- `BTC_AUTH_MODE=userpass`

rather than cookie auth. `run_local_world_sim_ethw.sh up` inherits the same BTC-side `world-sim`
bootstrap and wallet flows, so keeping explicit RPC credentials is the safer
default for:

- fresh bootstrap
- wallet-scoped `bitcoin-cli` calls
- `ord-server`
- deterministic recovery and replay

## 5. Single-Node Mining Model

The current ETHW path is based on the Ethash route, not a multi-validator PoA
scheme. For local simulation this means:

- a single ETHW node is sufficient to produce blocks
- there is no minimum multi-node requirement just to start mining
- extra nodes are only needed later for realism, peer sync, and topology tests

So the correct first milestone is a **single-node ETHW full-sim**.

## 6. Identity Strategy

For the first batch, the ETHW miner identity uses this precedence:

1. `ETHW_MINER_PRIVATE_KEY_HEX`
2. `ETHW_IDENTITY_MODE=deterministic-seed` with:
   - `ETHW_IDENTITY_SEED`, or
   - fallback to `WORLD_SIM_IDENTITY_SEED`
3. `ETHW_MINER_ADDRESS` only
4. no identity wiring

When a private key is available, the runtime should:

- import it into the local `geth` keystore once
- derive a stable ETH address
- write an identity marker in the ETHW data directory

When only an address is available, the runtime should:

- skip keystore import
- still write an identity marker
- expose the address as `ETHW_MINER_ADDRESS`

## 7. Runtime Behavior

The full-sim ETHW wrapper should:

1. prepare or load ETHW miner identity
2. fail closed if a persisted identity marker conflicts with the current inputs
3. export `ETHW_MINER_ADDRESS`
4. optionally append `--miner.etherbase ${ETHW_MINER_ADDRESS}` when the command
   does not already specify one
5. finally execute the provided `ETHW_COMMAND`

This keeps the runtime explicit while avoiding silent identity drift.

## 8. Second Batch: Protocol Identity Alignment

The second batch connects the deterministic ETHW miner identity to the BTC
world-sim mint flow.

The key rule is:

- when `run_local_world_sim_ethw.sh up` is used, the simulator should treat one
  configured world-sim agent as the protocol miner whose `eth_main` must match
  the ETHW miner address

The runtime contract is:

1. `run_local_world_sim_ethw.sh up` enables `ETHW_SIM_PROTOCOL_ALIGNMENT=1` by default
2. `start_ethw_full_sim.sh` writes the resolved ETHW miner identity to:
   - `${ETHW_DATA_DIR}/bootstrap/ethw-sim-identity.json`
3. `world-sim-bootstrap` and `world-sim-runner` mount the ETHW data volume
   read-only
4. `start_world_sim.sh` resolves `ETHW_MINER_ADDRESS` from:
   - explicit `ETHW_MINER_ADDRESS`, or
   - the ETHW identity marker
5. the simulator assigns that address to one stable agent:
   - `ETHW_MINER_AGENT_ID`
6. that agent's `mint` / `remint` actions use the aligned `eth_main`

This keeps the first ETHW full-sim milestone small:

- ETHW still remains a single-node local miner
- BTC world-sim remains the only source of protocol traffic
- but miner-pass `eth_main` stops drifting away from the ETHW mining identity

## 9. Future Phases

After the first batch is stable, the next ETHW-aware phases are:

1. **Bootstrap sequencing**
   - integrate `sourcedao-bootstrap` into the full-sim path

2. **ETHW-side simulation**
   - deterministic ETH accounts
   - funded user/demo accounts
   - contract calls and state progression

3. **Multi-node realism**
   - optional follower/full nodes
   - optional miner/bootnode split
