# Docker Tool Naming Plan

This document defines the naming system for repo-facing Docker helper tools.
The canonical local names in this file are already implemented. The remaining
goal is to keep the system scalable when the stack adds explicit `testnet` and
`mainnet` runtime profiles later.

## Status

The current repo-facing tool names are now:

- `run_local_console.sh`
- `run_local_runtime.sh`
- `run_local_world_sim.sh`
- `run_local_world_sim_ethw.sh`
- `run_local_bootstrap.sh`
- `run_local_full_sim.sh`
- `build_world_sim_images.sh`
- `smoke_bootstrap_stack.sh`

## Why The Current Names Do Not Scale

The current tool names are historically accurate, but they encode development
history more than operator intent.

Examples:

- `run_dev_full_runtime.sh`
  - mixes `dev` and `full`, but does not tell readers whether it includes
    simulation, bootstrap, or ETHW alignment
- `run_dev_full_sim.sh`
  - adds another `sim`, but that `sim` is really a profile that combines
    `world-sim + ETHW + SourceDAO bootstrap`
- `run_world_sim.sh up-full`
  - hides an important profile split in a subcommand instead of the name
- `run_sourcedao_bootstrap.sh`
  - is clearer than the others, but still speaks from one subsystem’s point of
    view rather than from the runtime profile point of view

The main problems are:

| Problem | Why it hurts |
| --- | --- |
| names are evolution-driven | they reflect how the stack grew, not how a user chooses a tool |
| `dev`, `full`, and `sim` are overloaded | readers cannot infer the service set or runtime behavior reliably |
| network tier is hidden | future `testnet` and `mainnet` helpers will look inconsistent if they are named ad hoc |
| profile boundaries are unclear | `world-sim up-full` and `dev-full-sim up` sound similar but are not the same scope |

## Naming Goals

The future naming system should satisfy these rules:

1. A tool name should describe a runtime profile or a utility role, not a
   development-history milestone.
2. The first semantic dimension should be deployment tier:
   - `local`
   - `testnet`
   - `mainnet`
3. The second semantic dimension should be runtime scenario:
   - `console`
   - `runtime`
   - `world-sim`
   - `bootstrap`
   - `joiner`
   - `full-sim`
4. Optional suffixes should capture meaningful variants only when necessary:
   - `ethw`
   - `readonly`
   - `smoke`
5. Utility tools should use verb-first naming, while runtime profile tools
   should use profile-first naming.

## Recommended Naming Model

Use two distinct naming grammars.

### 1. Runtime Profile Tools

Format:

`run_<tier>_<scenario>[_<variant>].sh`

Examples:

- `run_local_console.sh`
- `run_local_runtime.sh`
- `run_local_world_sim.sh`
- `run_local_world_sim_ethw.sh`
- `run_local_bootstrap.sh`
- `run_local_full_sim.sh`
- `run_testnet_runtime.sh`
- `run_mainnet_joiner.sh`

Why this works:

- `run_` makes it clear the tool starts or manages a stack profile
- `<tier>` makes `local / testnet / mainnet` the first-class distinction
- `<scenario>` captures the operator’s intent
- `<variant>` is only used when one scenario has multiple materially different
  forms

### 2. Utility Tools

Format:

`<verb>_<scope>[_<variant>].sh`

Examples:

- `build_world_sim_images.sh`
- `smoke_bootstrap_stack.sh`
- `check_world_sim_stack.sh`
- `inspect_bootstrap_state.sh`

Why this works:

- utilities are action-oriented rather than profile-oriented
- utility names should not pretend to be stack profiles

## Proposed Canonical Runtime Profiles

These names are recommended as the canonical profile vocabulary for docs, env
files, future wrappers, and any eventual dispatcher script.

| Canonical profile slug | Recommended wrapper name | Primary use | BTC role | ETHW role | SourceDAO role |
| --- | --- | --- | --- | --- | --- |
| `local-console` | `run_local_console.sh` | fastest console preview | regtest base services | not started | none |
| `local-runtime` | `run_local_runtime.sh` | local BTC + ord + ETHW runtime | regtest runtime | local dev ETHW node | none |
| `local-world-sim` | `run_local_world_sim.sh` | local BTC simulation | regtest + ord + world-sim | not started | none |
| `local-world-sim-ethw` | `run_local_world_sim_ethw.sh` | local BTC simulation plus ETHW alignment | regtest + ord + world-sim | local dev ETHW node | none |
| `local-bootstrap` | `run_local_bootstrap.sh` | local ETHW + SourceDAO bootstrap validation | regtest support services | bootstrap local chain | bootstrap enabled |
| `local-full-sim` | `run_local_full_sim.sh` | full local integrated simulation | regtest + ord + world-sim | bootstrap local chain + deterministic identity | bootstrap enabled |

## Current To Proposed Mapping

This is the key migration table.

| Current tool | Current problem | Proposed canonical name | Notes |
| --- | --- | --- | --- |
| `run_console_preview.sh` | emphasizes preview, but this is really a local console profile | `run_local_console.sh` | legacy name retired |
| `run_dev_full_runtime.sh` | `dev` and `full` are ambiguous | `run_local_runtime.sh` | better expresses "full local runtime, no sim" |
| `run_world_sim.sh up` | profile is hidden behind subcommand | `run_local_world_sim.sh` | `up` remains the default action |
| `run_world_sim.sh up-full` | `full` is ambiguous and only visible in subcommand | `run_local_world_sim_ethw.sh` | better reflects that ETHW is the actual variant |
| `run_sourcedao_bootstrap.sh` | subsystem-first naming | `run_local_bootstrap.sh` | bootstrap is the actual scenario; docs can mention SourceDAO explicitly |
| `run_dev_full_sim.sh` | bundles too many historical labels | `run_local_full_sim.sh` | this is the integrated local profile, so `full-sim` is acceptable here |

## Utility Rename Recommendations

| Current tool | Proposed name | Reason |
| --- | --- | --- |
| `build_world_sim_release_images.sh` | `build_world_sim_images.sh` | `release` is an implementation detail; the user cares that these are the packaged world-sim images |
| `run_container_smoke.sh` | `smoke_bootstrap_stack.sh` | this utility validates bootstrap/container wiring, not arbitrary containers |

These utility renames are also already applied in the current tree.

## Future Profiles For Testnet And Mainnet

The naming system should be able to absorb future public-network tools without
inventing a new grammar.

### Candidate Future Runtime Profiles

| Future profile slug | Recommended wrapper name | Expected BTC network | Expected ETHW network | Expected role |
| --- | --- | --- | --- | --- |
| `testnet-runtime` | `run_testnet_runtime.sh` | BTC public testnet | ETHW official testnet or staging chain | integrated public-network staging runtime |
| `testnet-joiner` | `run_testnet_joiner.sh` | BTC public testnet | ETHW official testnet or staging chain | join an existing staging network |
| `mainnet-runtime` | `run_mainnet_runtime.sh` | BTC mainnet | ETHW mainnet | production-like runtime bring-up |
| `mainnet-joiner` | `run_mainnet_joiner.sh` | BTC mainnet | ETHW mainnet | production joiner / node attach path |

### Important Naming Rule

The wrapper name should reflect the profile tier, while the actual chain-level
details continue to live in the env file and docs.

That means:

- the name says `local / testnet / mainnet`
- the env file says exactly which BTC network and which ETHW network are used

This prevents names from becoming too long while still making public-network
differences first-class.

## Migration Status

The repo has already completed:

- Phase 1: define canonical names
- Phase 2: rename the tool files and move docs to canonical names

The current remaining optional step is:

### Phase 3. Optional Dispatcher Unification

If the tool set grows much further, add a single dispatcher:

`run_profile.sh <profile> <action>`

Examples:

```bash
docker/scripts/run_profile.sh local-runtime up
docker/scripts/run_profile.sh local-world-sim up
docker/scripts/run_profile.sh mainnet-joiner up
```

This would reduce script sprawl while keeping the profile vocabulary stable.

## Recommendation

Current recommendation:

- keep the canonical names stable
- avoid reintroducing history-driven aliases
- add new public-network tools by following the same naming grammar

Preferred canonical runtime names:

- `run_local_console.sh`
- `run_local_runtime.sh`
- `run_local_world_sim.sh`
- `run_local_world_sim_ethw.sh`
- `run_local_bootstrap.sh`
- `run_local_full_sim.sh`

This keeps the current local stack understandable and gives the repo a naming
grammar that can scale to:

- `run_testnet_runtime.sh`
- `run_testnet_joiner.sh`
- `run_mainnet_runtime.sh`
- `run_mainnet_joiner.sh`
