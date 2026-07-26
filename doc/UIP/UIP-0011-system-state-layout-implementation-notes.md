# USDB Reserved System State Layout

Status: Frozen for development v1 implementation

Related: UIP-0011, UIP-0012, UIP-0013, UIP-0014, UIP-0015

# Purpose

This note freezes the shared USDB-chain account and storage namespace consumed by
UIP-0011 through UIP-0014. It is an implementation companion to those UIPs, not
an independently activated policy.

The address and slot derivation are consensus inputs. A public-network release
must include the exact values below in its genesis artifact and golden-vector
tests.

# Reserved Account

```text
USDB_SYSTEM_STATE_ADDRESS = 0x0000000000000000000000000000000000001000
USDB_SYSTEM_STATE_NONCE   = 1
USDB_SYSTEM_STATE_CODE    = empty
USDB_SYSTEM_STATE_BALANCE = 0
SYSTEM_STATE_SCHEMA_VERSION = 1
```

Rules:

- The address is identical on every USDB-chain network and is not a runtime
  parameter.
- Nonce `1` prevents `CREATE` and `CREATE2` from installing code at the
  reserved account.
- The account never executes EVM code. Consensus writes its storage directly
  through deterministic state transitions.
- Ordinary transactions, user contracts, SourceDAO and Dividend cannot modify
  the reserved storage.
- Native token transfers sent to this address are locked and do not alter the
  meaning of any protocol slot.

# Static Slot Derivation

Every static slot is:

```text
slot = keccak256(UTF8("usdb.system.state.v1/<component>/<field>"))
```

The exact v1 domains and results are:

| Name | Domain | Slot |
| --- | --- | --- |
| `SYSTEM_STATE_SCHEMA_VERSION_SLOT` | `usdb.system.state.v1/meta/schema-version` | `0x173335b1b35d7ee82b9e595fe4798c8e777ab08ec72cb8ea8a8035ee1fade3b1` |
| `ISSUED_USDB_ATOMS_SLOT` | `usdb.system.state.v1/reward/issued-usdb-atoms` | `0xdd1651483272028cad87b8ab291a694a9deb1d7f6b60efe175f823c406233da2` |
| `PRICE_ATOMS_PER_BTC_SLOT` | `usdb.system.state.v1/price/price-atoms-per-btc` | `0xdf22b00cd5b1ebfe143e44347701e86394b9867790d8d631d43ef36dd099f884` |
| `REAL_PRICE_ATOMS_PER_BTC_SLOT` | `usdb.system.state.v1/price/real-price-atoms-per-btc` | `0xca9c2c48cf84f8c36afc338940b0e06484e790e7190e255a57245056399bb792` |
| `PRICE_POLICY_VERSION_SLOT` | `usdb.system.state.v1/price/policy-version` | `0xc65fb6e80dc7887c39c44824450f50076c21ffb398bc3abc8ec122d277f7ce03` |
| `PRICE_SOURCE_KIND_SLOT` | `usdb.system.state.v1/price/source-kind` | `0x93fbc84343f98a946b33b6067ae017273d92029de3e58c3b3c6d37fb033cac9a` |
| `PRICE_POLICY_RANGE_ID_SLOT` | `usdb.system.state.v1/price/policy-range-id` | `0xc3faa41e87f1db8d882f1a24fd36bf5f7f873e141845019088d03d0e2f487697` |
| `K_WINDOW_SUM_SLOT` | `usdb.system.state.v1/collaboration/window-sum` | `0xa05125c861ef555402b28fe982e4e36ddd9572a49576081d06ad23fbdcd9a3ae` |
| `K_WINDOW_COUNT_SLOT` | `usdb.system.state.v1/collaboration/window-count` | `0x40db96c2e761efb468bcae40739cb9d71d15e53f4b46a977d213476493a0ecea` |
| `K_WINDOW_CURSOR_SLOT` | `usdb.system.state.v1/collaboration/window-cursor` | `0xc71798c59dae3ab826f28ffa3db501face181bd2d88225baadcb87ea950c53b2` |
| `K_CE_RING_SLOT_BASE` | `usdb.system.state.v1/collaboration/ce-ring` | `0x0c0b1b7c7641949e2f45575f48d889a70298842709e50c1070010b910fb3bc31` |
| `K_LAST_CE_SLOT` | `usdb.system.state.v1/collaboration/last-ce` | `0x1d2465ef2bfb872650e27eeb6a1327cb569d58e4fd2c4867eb4b8f38b922905c` |
| `K_LAST_AE_SLOT` | `usdb.system.state.v1/collaboration/last-ae` | `0xb4d89df049af3068c7073e80bf4918d5606bffb9df517e96c1f996f942c38f58` |
| `K_LAST_K_BPS_SLOT` | `usdb.system.state.v1/collaboration/last-k-bps` | `0x53264b8f3aab69de54c5a4ecadabdbff09c07064034e8fcfdb79056a55dd9954` |
| `QUOTE_POLICY_VERSION_SLOT` | `usdb.system.state.v1/quote/policy-version` | `0x06ed1ff69c0a83234a648936403718a01fd0c0e6caabe4eea61d7735f63db832` |
| `LEADER_QUOTE_WINDOW_BLOCKS_SLOT` | `usdb.system.state.v1/quote/window-blocks` | `0x34d422b9f7b2447c9ad568159320894837919eacfd196ee5c5ede41376c56358` |
| `LEADER_LAST_VALID_QUOTE_BLOCK_MAP_BASE` | `usdb.system.state.v1/quote/leader-last-valid-block` | `0x9f4c948c72431d7f43911f1f1231509866c87a43729568fdf10a86f9291b9cba` |

# Dynamic Slot Derivation

Dynamic values use the Solidity-compatible mapping rule:

```text
mapping_slot(key32, base_slot)
    = keccak256(key32 || base_slot)
```

For the UIP-0012 ring:

```text
key32 = uint256(index) encoded as 32-byte big-endian
K_CE_RING_SLOT(index) = mapping_slot(key32, K_CE_RING_SLOT_BASE)
```

For UIP-0014 quote state:

```text
txid[32] = canonical display txid hex decoded left-to-right
           (no Bitcoin internal-byte-order reversal)
pass_id_bytes = txid[32] || inscription_index_u32_be
quote_subject_key = keccak256(pass_id_bytes)
LEADER_LAST_VALID_QUOTE_BLOCK_SLOT(pass_id)
    = mapping_slot(quote_subject_key, LEADER_LAST_VALID_QUOTE_BLOCK_MAP_BASE)
```

Golden examples:

```text
K_CE_RING_SLOT(0)
  = 0xb219b1bbf4732ed92adf0117a200e771f975c506d1774f83bd9ceca8d40b47af

pass_id = 0101010101010101010101010101010101010101010101010101010101010101i7
quote_subject_key
  = 0x843e3be447dd1809885dc50b1f54731391166abab16119510ad06d4bb586e422
LEADER_LAST_VALID_QUOTE_BLOCK_SLOT(pass_id)
  = 0x50bdd3b511e9a1d9f70ca8cc57c354965ef411fcac0be5754bf0eca94ce0de25
```

# UIP-0013 Fixed-Price Range Identity

The v1 range ID stored in `PRICE_POLICY_RANGE_ID_SLOT` is:

```text
keccak256(
  UTF8("usdb.price.policy.range:v1") || 0x00 ||
  uint256_be(chain_id) ||
  uint64_be(start_block) ||
  uint32_be(price_policy_version) ||
  uint32_be(price_source_kind) ||
  uint256_be(const_price_atoms_per_btc)
)
```

For development chain ID `20260323`, start block `0`, policy/source `1`, and
`100000000000000000000000` atoms/BTC:

```text
PRICE_POLICY_RANGE_ID
  = 0x2ae45cafae84cc892d1d4354f02a0869f97dfd6ca2c757ba511c57680b8bfaf4
```

The v1 constant is part of the policy implementation. A different fixed price
requires a new price policy version and must not reuse the v1 identity.

# Dividend Readiness Slot

UIP-0010 readiness is intentionally stored in the Dividend system contract,
not in `USDB_SYSTEM_STATE_ADDRESS`:

```text
DIVIDEND_BOOTSTRAP_FINALIZED_SLOT
  = keccak256(UTF8("sourcedao.dividend.bootstrap-finalized:v1"))
  = 0x7d8bb76c5e489191d3f481f0b7ade016df922a8ec91d3eb9c93c07ee5a337054
```

Fee policy v1 requires both the chain-config `DividendCodeHash` and this slot to
equal `uint256(1)`. Local bootstrap files and service readiness are not
consensus inputs.

# Genesis

Every USDB genesis must create the reserved account and initialize:

```text
SYSTEM_STATE_SCHEMA_VERSION_SLOT = 1
ISSUED_USDB_ATOMS_SLOT = checked_sum(all genesis alloc balances)
```

The issued-supply sum includes SourceDAO, Dividend, bootstrap admin and every
other genesis-funded account. It uses unsigned 256-bit arithmetic and genesis
construction fails on a negative, missing or overflowing balance.

Policy slots whose policy version is `0` remain canonical zero and may be absent
from the storage trie. Development v1 initializes the UIP-0013 fixed-price
slots from genesis. UIP-0012 K slots start at canonical zero and are populated
one sample per reward block. UIP-0014 remains zero until its policy activates.

# Upgrade Rule

Existing domain strings and slot values are immutable. A future incompatible
layout uses a new schema version and new domain prefix. It must not reinterpret
or overwrite a v1 slot with a different type or semantic meaning.
