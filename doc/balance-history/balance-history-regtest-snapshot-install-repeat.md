# Balance-History Regtest: Snapshot Install Repeat

本文档说明同一 snapshot 对同一 root 重复安装的场景，目标是验证 install-snapshot 的幂等性、backup 累积行为，以及重复安装后是否仍可正常启动并继续追块。

## 覆盖目标

1. 首次安装 snapshot 后，目标 root 的 stable state 与 snapshot 一致。
2. 对同一 root 再次安装同一 snapshot 后，stable state 仍保持一致，不会留下 staging 残留。
3. 每次安装都会保留一份此前 live DB 的 backup，重复安装后 backup 数量应继续增加。
4. 重复安装完成后，服务仍可继续追块并反映新的 live UTXO 与余额。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
```

## 验收标准

脚本成功时会输出 `Snapshot install repeat test succeeded.`。