# Balance-History Regtest: Snapshot Install Corrupt

本文档说明“snapshot 文件存在且 hash 正确，但内容已损坏”的场景，目标是验证 install-snapshot 在打开或读取损坏 snapshot DB 失败时不会污染目标 root。

## 覆盖目标

1. 损坏后的 snapshot 文件即使使用它自己的正确 hash，也会在安装阶段失败。
2. 失败后不会留下 staging 残留，也不会产生 live DB backup。
3. 失败后重启目标服务，原有 stable state、地址余额和 live UTXO 视图保持不变。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_install_corrupt.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_install_corrupt.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_install_corrupt.sh
src/btc/balance-history/scripts/regtest_snapshot_install_corrupt.sh
```

## 验收标准

脚本成功时会输出 `Snapshot install corrupt test succeeded.`。