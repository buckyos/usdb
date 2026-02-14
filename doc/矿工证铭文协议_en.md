# Miner Certificate Inscription Protocol (Draft)

> Source: `https://github.com/buckyos/usdb/issues/5` and its discussion comments.
> Status: Draft, intended to align indexer and product implementation behavior.

## 1. Protocol Goals

- Use one `mint` inscription for miner certificate creation, activation, and energy inheritance.
- Support transferable certificates under the BTC UTXO model while preventing dust attacks and energy double spending.
- Bind a primary ETH address and an optional collaborator ETH address for future reward/collaboration extensions.

## 2. Inscription Data Format

All miner-certificate operations are represented by JSON inscriptions with `op` fixed to `mint`.

### 2.1 Field Definitions

| Field | Type | Required | Description | Constraints |
| -- | -- | -- | -- | -- |
| `p` | string | Yes | Protocol identifier | Must be `"usdb"` |
| `op` | string | Yes | Operation type | Must be `"mint"` |
| `eth_main` | string | Yes | Main ETH address for rewards | Must be a valid EVM address |
| `eth_collab` | string | No | Collaborator ETH address for collaboration/reward split extensions | Empty or valid EVM address |
| `prev` | string[] | No | Parent inscriptions for inheritance | Each item must be a valid inscription ID |

### 2.2 Example

```json
{
  "p": "usdb",
  "op": "mint",
  "eth_main": "0x1234...NewEthAddr...",
  "eth_collab": "0x5678...CollabAddr...",
  "prev": [
    "old_inscription_id_a",
    "old_inscription_id_b"
  ]
}
```

## 3. State Model

The indexer maintains the following states for miner certificates:

- `Active`: currently effective certificate used for miner identity and energy settlement.
- `Dormant`: inactive state, for example after transfer.
- `Consumed`: energy already inherited by a newer certificate and cannot be inherited again.

## 4. Core Rules

### A1. Mint Right Implies Activation Right

- For BTC address `A`, the active certificate can only be the latest valid `mint` inscription created by `A` itself.
- Passively received inscriptions do not become active automatically.

Security impact: prevents dust attacks from hijacking activation status.

### A2. Transfer Implies Dormancy

- Any `Active` inscription becomes `Dormant` immediately once its UTXO is transferred.
- The new holder must mint a new inscription on their own address (optionally with `prev`) to reactivate.

Security impact: enforces explicit ownership transition and prevents implicit state drift.

### A3. Atomic Energy Consumption

- When `I_new` references `I_old` via `prev`, the indexer must:
  1. finalize the inheritable energy of `I_old` at the reference point,
  2. attach that finalized value to `I_new` state/metadata,
  3. mark `I_old` as `Consumed`.

Security impact: prevents energy double spending.

### A4. Single-Inscription Mechanism

- The protocol uses a single-inscription approach (`mint` + inheritance) rather than `mint + active` dual inscriptions.
- The indexer must distinguish self-minted activation vs passive holding and assign correct initial state.

## 5. Key Flows

### 5.1 First Activation / ETH Address Update

1. User mints a new `mint` inscription `I_new` with updated `eth_main` / `eth_collab`.
2. If inheritance is needed, set `prev` to old inscription IDs.
3. `I_new` becomes the current `Active` inscription; referenced old inscriptions become `Consumed` according to rules.

### 5.2 Transfer and Reactivation

1. Address `A` transfers old inscription `I_old` to `B`; `I_old` turns `Dormant` immediately.
2. `B` mints `I_new` on `B`'s own address with `prev = [I_old]`.
3. `I_new` becomes `Active`, inherits energy, and binds `B`'s ETH addresses.

## 6. Energy Calculation and Inheritance Recommendations

> These are current discussion outcomes from the issue and should remain configurable before mainnet finalization.

- Settlement path: indexer must provide miner energy at the target block height.
- Explorer/audit path: support querying miner energy at arbitrary historical block heights.
- Inheritance decay: recommended default decay rate is `5%` (inherited energy = inheritable energy × `0.95`).

## 7. Minimum Indexer Validation Requirements

- Process only inscriptions matching `p == "usdb" && op == "mint"`.
- `eth_main` is mandatory and must be valid; `eth_collab` must be valid when present.
- Every `prev` reference must exist, be accessible, and not be previously consumed.
- Inheritance over a referenced inscription must be transactional or equivalently atomic.
- State transitions (`Active -> Dormant -> Consumed`) must include full-context logs for traceability.

## 8. Open Items

- Detailed algorithm for `eth_collab` reward split and energy boost behavior.
- Exact energy growth formula (BTC balance, holding duration, sampling period).
- Whether to add a protocol version field (for example `v`) for forward compatibility.
