# Live Ord Validator Block-Body Payload-Version Upgrade Reorg

## 目标

验证 mixed payload upgrade 窗口下，旧 `v1.0` payload 与新 `v1.1` payload 在 same-height replacement 后能够稳定分流。

## 场景

1. 在高度 `H` 生成旧 `v1.0` 单 pass payload。
2. 在高度 `H+1` 生成新 `v1.1` candidate-set payload。
3. 对 `H+1` 执行 same-height replacement。
4. 回放两代 payload。

## 期望

- `H` 上的旧 `v1.0` payload 仍按历史 context 成立。
- `H+1` 上的新 `v1.1` payload 稳定返回 `SNAPSHOT_ID_MISMATCH`。
- mixed schema 与 reorg 叠加后，错误分流仍然清晰。
