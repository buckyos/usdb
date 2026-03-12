# Balance-History Regtest: Loader Switching

本文档说明 `balance-history` 在不同追块落后程度下的 BTC client 选择场景，目标是验证冷启动大幅落后时会走 `LocalLoader`，而在已同步后的少量增量追块阶段会回退到普通 RPC client。

## 覆盖目标

1. 首次启动时，链高明显高于本地 DB 高度，且超过 `local_loader_threshold`，服务会选择 `LocalLoader`。
2. `LocalLoader` 完成冷启动追块后，地址余额查询结果正确。
3. 服务重启后只需追少量新块时，会改走 RPC BTC client。
4. RPC 增量追块后，新旧地址余额都正确，且 block commit 收敛到新的 tip hash。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_loader_switch.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_loader_switch.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_loader_switch.sh
src/btc/balance-history/scripts/regtest_loader_switch.sh
```

## 验收标准

脚本成功时会输出 `Loader switch test succeeded.`。