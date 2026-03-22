# USDB Indexer Regtest: Live Ord Validator Block-Body Two-Pass Real Energy Advantage

## 1. 目标

这条场景验证多 `pass` validator payload 不只是能覆盖 `energy=0` 时的 tie-break，还能覆盖真实 `energy` 差异驱动的 winner 选择。

核心要证明两件事：

1. 在历史高度 `H`，同一候选集合里的 winner 确实由更高 `energy` 决定。
2. 后续 BTC 继续推进并让另一张 pass 的当前 `energy` 反超后，旧 payload 仍必须按 `H` 的历史上下文成立，而新 payload 会切到新的 winner。

## 2. 脚本入口

- 脚本：[src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_two_pass_energy_advantage.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_two_pass_energy_advantage.sh)

## 3. 场景步骤

1. 预挖 regtest 区块并启动 `ord server`
2. mint `pass1`
3. 给 `pass1` owner 地址追加真实 BTC balance，并等待数个增长块
4. 稍后 mint `pass2`
5. 在竞争高度 `H`：
   - 读取 `pass1.energy`
   - 读取 `pass2.energy`
   - 断言 `pass1.energy > pass2.energy`
6. 基于高度 `H` 写出 multi-pass validator payload，winner 固定为 `pass1`
7. 再给 `pass2` owner 地址追加更大的真实 BTC balance，并等待增长块
8. 在新的 head：
   - 读取 `pass1.energy`
   - 读取 `pass2.energy`
   - 断言 `pass2.energy > pass1.energy`
9. 再次校验：
   - 高度 `H` 的旧 payload 仍通过
   - 新高度生成的新 payload 切换到 `pass2`

## 4. 关键断言

### 4.1 历史 winner 由真实 energy 决定

在高度 `H`：

- `pass1.energy > 0`
- `pass1.energy > pass2.energy`
- payload 里的 `miner_selection.inscription_id == pass1`

### 4.2 当前赢家翻转不影响历史 payload

在更高 head：

- `pass2.energy > pass1.energy`
- 基于新 head 生成的新 payload 里的 winner 变成 `pass2`
- 旧 payload 仍按 `H` 的 `external_state` 验证通过

## 5. 价值

相比 `two-pass competition` 只覆盖 `max_energy + inscription_id` tie-break，这条场景更贴近真实 ETHW validator 选择逻辑：

- winner 由真实 `energy` 优势决定
- winner 变化来自真实链上余额和增长块，不是手工篡改 payload
- validator 必须同时接受：
  - 历史高度的旧 winner
  - 当前高度的新 winner

## 6. 通过标准

脚本结束时输出：

```text
USDB validator block-body two-pass real-energy-advantage test succeeded.
```

同时日志会打印：

- `h`
- `pass1_energy_h`
- `pass2_energy_h`
- `flipped_height`
- `pass1_energy_flipped`
- `pass2_energy_flipped`

用于确认 winner 翻转确实来自真实 `energy` 变化。
