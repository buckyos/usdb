# USDB Indexer Regtest: Live Ord Validator Block-Body Two-Pass Competition

## 1. 目标

这条场景把 validator block-body 从“单张 pass 的历史校验”推进到“同一历史高度下多张候选 pass 的相对关系校验”。

目标是验证：

1. 出块方可以在 BTC 高度 `H` 固定一份包含 `winner + candidates` 的 validator block-body payload。
2. 验证方可以仅依赖 payload 和 BTC RPC，在同一份历史 `external_state` 下重查多张 pass。
3. validator 不仅能重放单张 pass 的历史状态，还能证明 `winner` 在 `H` 时满足候选排序规则。
4. 即使后续块让 winner 本身发生真实状态变化，旧 payload 仍应按 `H` 的视图校验通过。

## 2. 场景设计

### 2.1 候选集合

使用两张 pass：

- `pass1`
  - 先 mint
  - 在第二张 pass 出现前先额外存活若干空块
  - 在竞争高度 `H` 更“老”
- `pass2`
  - 后 mint
  - 在竞争高度 `H` 仍较“新”

当前 regtest 下，两张 pass 的 energy 可能打平。因此脚本不硬编码 `pass1` 一定获胜，而是按 payload 声明的选择规则动态计算 winner：

- `max_energy`
- 若 energy 相同，则按 `inscription_id` 字典序升序 tie-break

### 2.2 Payload 结构

payload 仍复用当前 block-body 结构：

- `external_state`
- `miner_selection`
- `candidate_passes`
- `selection_rule`

其中：

- `miner_selection` 记录 winner
- `candidate_passes` 记录同一高度的两张候选 pass
- `selection_rule.kind = max_energy`
- `selection_rule.tie_breaker = inscription_id_lexicographic_asc`

### 2.3 后续状态推进

在生成 payload 之后，再让当时的 winner 在 `H+1` 发生真实变化：

- `transfer(winner)`

这会让当前 head 上的 winner 从 `active -> dormant`，但不应影响 validator 对旧 payload 的历史校验。

## 3. 验证要点

### 3.1 在竞争高度 `H`

1. 两张 pass 都能按同一份历史 `external_state` 被重查。
2. `candidate_passes` 的 winner 满足 `max_energy + inscription_id_lexicographic_asc`。
3. `miner_selection.inscription_id` 与该规则计算出的 winner 一致。
4. validator 在历史视图下能重放出 winner 与 candidate 集合的一致关系。

### 3.2 进入 `H+1`

1. 当前 head 上 winner `state == dormant`
2. 当前 head 上 loser `state == active`
3. 旧 payload 仍然按 `H` 验证通过

### 3.3 继续前进到 `H+2`

1. BTC head 再前进一个空块
2. 旧 payload 仍然按 `H` 验证通过

## 4. 对应脚本

- [regtest_live_ord_validator_block_body_two_pass_competition.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_two_pass_competition.sh)

## 5. 预期价值

这条场景补齐了单 pass validator block-body 尚未覆盖的一层：

- 不只是“这张 pass 在 `H` 时是不是这个状态”
- 而是“在 `H` 时为什么最后选中的是这张，而不是另一张候选 pass”

这会更接近 ETHW validator 对真实 block body 的消费方式。
