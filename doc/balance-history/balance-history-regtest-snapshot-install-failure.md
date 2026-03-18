# Balance-History Regtest: Snapshot Install Failure

本文档说明一个 snapshot 安装异常场景：安装在校验阶段失败后，目标 root 里原有的 live DB 不应被污染，也不应留下 staging 或 backup 目录。

## 覆盖目标

该场景覆盖以下失败语义：

1. `install-snapshot --hash <wrong>` 在 hash mismatch 时必须失败。
2. `install-snapshot --file <missing>` 在快照文件不存在时必须失败。
3. 上述失败都不应创建 `snapshot_install_staging_*` 临时目录。
4. 上述失败都不应创建 `db_backup_snapshot_install_*` 备份目录。
5. 失败后重启目标服务，原有 stable height、stable block hash、余额和 live UTXO 视图必须保持不变。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_install_failure.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_install_failure.sh)

## 场景结构

1. 源 root 索引到较新高度并生成 snapshot。
2. 目标 root 通过 `max_sync_block_height` 固定在更旧的 stable height，形成与 snapshot 明显不同的可观测状态。
3. 对目标 root 先后执行两次失败安装：
   - 错误 hash
   - 缺失文件
4. 每次失败后都重启目标服务，确认其 stable state 仍然停留在旧高度，不会被较新 snapshot 悄悄覆盖。

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_install_failure.sh
src/btc/balance-history/scripts/regtest_snapshot_install_failure.sh
```

如需并行运行，请覆盖端口：

```bash
WORK_DIR=/tmp/usdb-bh-snapshot-install-failure-debug \
BTC_RPC_PORT=29532 \
BTC_P2P_PORT=29533 \
BH_RPC_PORT=29510 \
TARGET_BH_RPC_PORT=29511 \
src/btc/balance-history/scripts/regtest_snapshot_install_failure.sh
```

## 验收标准

脚本成功时会输出 `Snapshot install failure test succeeded.`。若失败，会打印 bitcoind 与目标服务日志尾部，方便判断是校验失败行为不对，还是失败后出现了 live DB 污染。