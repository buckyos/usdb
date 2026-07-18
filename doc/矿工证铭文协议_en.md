# Miner Pass Inscription Protocol

> Status: superseded by the UIP protocol set. This document is an implementation entry point and no longer defines independent consensus rules.

The original issue draft has been split and finalized into these specifications:

- [UIP-0001: Miner Pass Inscription](UIP/UIP-0001-miner-pass-inscription.md): v1 mint JSON, standard/collab kinds, Leader bindings, and `prev` semantics.
- [UIP-0002: Miner Pass State Machine](UIP/UIP-0002-pass-state-machine.md): Active, Dormant, Consumed, Burned, and Invalid states plus block-level settlement.
- [UIP-0003: Miner Pass Energy Formula](UIP/UIP-0003-pass-energy-formula.md): raw energy, inheritance discount, penalties, and saturating arithmetic.
- [UIP-0004: Collab Leader Effective Energy](UIP/UIP-0004-collab-leader-effective-energy.md): Leader resolution, collab contribution, and effective energy.
- [UIP-0005: Level and Real Difficulty](UIP/UIP-0005-level-and-real-difficulty.md): level and difficulty factor derivation.
- [UIP-0006: USDB Economic State View](UIP/UIP-0006-usdb-economic-state-view.md): auditable and frozen external query contracts.

## v1 Mint Entry Point

Every mint inscription must contain:

```json
{
  "p": "usdb",
  "op": "mint",
  "v": 1
}
```

Identity fields must match exactly one shape:

- Standard pass: a valid EVM `usdb_main` and no Leader binding field.
- Fixed-Leader collab pass: a valid `leader_pass_id` and neither `usdb_main` nor another Leader binding field.
- Address-Leader collab pass: a valid `leader_btc_addr` on the active BTC network and neither `usdb_main` nor another Leader binding field.

`prev` is an optional array of inscription ids. Existence, owner, state, duplicate-reference, inheritance-discount, and atomic-consumption rules are defined by UIP-0001, UIP-0002, and UIP-0003.

Implementations, tests, and reviews must use the relevant UIP text as the only normative source. This document must not be treated as a compatibility protocol.
