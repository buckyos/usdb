# Live Ord Validator Block-Body Payload-Version Upgrade Restart

## 目标

验证两代 validator payload 在服务重启后仍可按历史 context 正确回放，证明升级窗口不依赖进程内缓存。

## 场景

1. 先生成 `v1.0` 单 pass payload。
2. 再生成 `v1.1` candidate-set payload。
3. 重启 `balance-history` 和 `usdb-indexer`。
4. 在重启后的同一 tip 上重新回放两代 payload。

## 期望

- mixed payload 在重启后仍能通过。
- `state ref` 回放和 payload schema 路径都不依赖进程内临时状态。
