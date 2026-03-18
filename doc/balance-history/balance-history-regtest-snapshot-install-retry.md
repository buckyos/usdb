# Balance-History Regtest: Snapshot Install Retry

本文档说明 snapshot 安装失败后重试成功的场景，目标是验证失败不会污染 live DB，且后续可以用正确参数重试成功并继续追块。

## 覆盖目标

1. 错误 hash 的安装尝试会失败，且不会留下 staging 残留。
2. 失败后使用正确 hash 重试可以成功安装。
3. 成功安装后会保留一份旧 live DB backup，表示原子切换生效。
4. 成功安装后的目标 root 对外 stable state 与 snapshot 一致。
5. 成功安装后的服务可以继续同步后续新块。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_install_retry.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_install_retry.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_install_retry.sh
src/btc/balance-history/scripts/regtest_snapshot_install_retry.sh
```

## 验收标准

脚本成功时会输出 `Snapshot install retry test succeeded.`。
