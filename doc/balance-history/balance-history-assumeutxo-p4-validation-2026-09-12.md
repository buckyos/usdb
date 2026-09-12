# AssumeUTXO P4 主网验证复核记录

日期：2026-09-12。实验目录：`/data/usdb-assumeutxo-validation/p4-mainnet-935000-to-963800`。

结论：**P4 主网导入、重放和全量逻辑投影对比通过。**
从935000 Core快照导入的完整UTXO及余额，经935001–963800重放后，与既有完整重放core参考在963800一致。
P5的全量UTXO、非零当前余额和逐块commit子项已有通过证据；查询契约、registry覆盖、下游state-ref及部署集成尚未完成。

后续进展：同日已完成[P5离线语义与独立regtest验收](./balance-history-assumeutxo-p5-validation-2026-09-12.md)；
下文保留P4复核时的证据及阶段边界，部署集成仍留待P6/P7。

本次复核读取既有报告和完整阶段日志，独立检查参考SQLite元数据及首尾commit、发布记录中的参考文件hash、
当前Bitcoin canonical目标hash，并重新运行6项小规模测试。未再次运行约12分钟的全量compare，也未再次扫描原始快照或计算37GB参考文件hash；
这两项内容hash依赖本次成功运行保存的校验结果。现有日志没有保存逐命令shell退出码或 `/usr/bin/time -v` 统计，
不能把本次结论描述为“追溯确认每条历史shell命令退出码均为0”；核心判定依据是最终成功报告、完整扫描行数/hash、成功结束日志及本次交叉核验。

## 1. 分阶段结果

| 阶段 | 已核对证据 | 结果 |
| --- | --- | --- |
| 扫描 | 164,241,311个UTXO，113,879,165个交易组，9,387,990,306字节；文件SHA-256和Core内容hash匹配 | 通过 |
| 导入 | `import-result.json`与`state/assumeutxo-source.json`一致，源身份与scan/input/仓库身份文件一致 | 通过 |
| 重放 | 935000→963800；1,440批，每批20块，高度连续，共28,800块；成功运行段无后续错误 | 通过 |
| 全量对比 | 三项双方行数、完整有序投影SHA-256均一致，`equal=true` | 通过 |
| 当前目标身份 | 原节点返回的963800 canonical hash与参考core及replay报告一致 | 通过 |
| 小规模测试 | 本次重跑6项AssumeUTXO测试，0失败 | 通过 |

比对的是完整逻辑行流，不是抽样或仅比较总金额。

## 2. 固定身份

| 字段 | 值 |
| --- | --- |
| 导入基线 | 935000 |
| 基线BTC hash | `0000000000000000000147034958af1652b2b91bba607beacc5e72a56f0fb5ee` |
| 源快照文件SHA-256 | `e572ddbe456d254f05fb004cebe225bdb3656074b66f0e9b1c7fa83e1301d486` |
| Core hash_serialized_3 | `e4b90ef9eae834f56c4b64d2d50143cee10ad87994c614d7d04125e2a6025050` |
| 基线USDB commit | `6108f77e4abaafbc3a7a246024e942483c18fb37a3c710209ea6294c5617fe81` |
| 参考core文件SHA-256 | `3e3490ac19521647a8513a0ef2961607df4456ba3e3ea6922c1ba750f7fbea61` |
| 参考core snapshot ID | `7b1cf69a6922b55a90f378c3d03daa8e64c4dba48145cae6e0989be5c9730562` |
| 目标高度 | 963800 |
| 目标BTC hash | `000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551` |
| 目标USDB commit | `d5981b06db4e1f9d12f3a9e8b66da45001d0b38d3e2a689de3251542922f1e75` |

参考文件路径由实验 `input.json` 固定，对应仓库 `docker/networks/testnet-v0/snapshots/balance-history-snapshot-release-record.json` 中的core文件hash。
参考snapshot ID是旧core的身份，本次不据此宣称新库已通过全部下游state-ref验收。

## 3. 全量对比结果

| 投影 | 候选行数 | 参考行数 | 双方一致的SHA-256 |
| --- | ---: | ---: | --- |
| 活UTXO：outpoint/script_hash/value | 165,748,439 | 165,748,439 | `86cba93334b2a6b6862a00d070617a0bdfded56b8eec2a25c41c2dcff82faedd` |
| 非零当前余额：script_hash/balance | 59,356,343 | 59,356,343 | `7e9e427332cf8bf95a52cd8689e2b17c732593b69a3f7f062c03dd93ab92449b` |
| 935000–963800逐块commit记录 | 28,801 | 28,801 | `f4d1338b39c262dc9a4ffa15152a4d9943fdb35d6c22a70f619e47e2d0ffafaf` |

UTXO投影保留零金额输出；余额投影双方排除零余额行，并不比较基线前最后变动高度/delta。
commit投影包括高度、BTC hash、balance delta root及rolling commit，基线锚点也在比较范围内。

## 4. 错误日志与状态解释

`p4-mainnet-935000-to-963800.replay.log` 通过 `tee -a` 追加，开头保留两次旧错误：

```text
Failed to update BTC RPC client: I/O error: No such file or directory (os error 2)
```

这是之前供块节点磁盘不足退出后cookie消失导致的失败。两次错误都在成功重放开始之前；
成功段从日志第15行开始，到第1456行 `AssumeUTXO replay finished: height=963800` 结束。
之后 `comparison-result.json` 记录三项一致，没有新的失败记录。

`replay-progress.json` 最后仍为 `status=running,height=963800`，因为当前工具保留最后一批进度，
将完成结果另写 `replay-result.json`。最终判定使用后者的 `status=pass` 及compare结果。
本次宿主机进程检查确认没有P4工具进程仍在运行。无需为了消除旧错误日志或这个进度字段重新导入/重放。

## 5. 时间、空间与证据位置

| 项目 | 观测值 | 口径 |
| --- | --- | --- |
| 独立扫描 | 32.1秒 | scan报告计时 |
| 导入 | 1945.5秒，约32分26秒 | 解码/写库阶段；参考文件校验另计 |
| 重放 | 3820.3秒，约1小时3分40秒 | replay报告计时；参考文件校验另计 |
| 全量对比 | 约12分2秒 | 单次compare日志创建至最后修改的文件时间窗口估计，非time工具测量 |
| 导入后state | 18,779,533,312字节，约17.49 GiB | 上次故障排查观测 |
| 重放后state | 31,401,349,120字节，约29.24 GiB | 本次 `du -s -B1`；非峰值 |

供块使用现有28.1全量节点，不代表公网独立冷启动时间。新库只建立live及基线后脚本覆盖，
不能将其磁盘占用直接当成旧完整历史registry的等覆盖替代。峰值RSS、峰值磁盘、原下载耗时未记录。

所有原始阶段日志位于实验目录的同级，最终报告位于实验目录内：

- `p4-mainnet-935000-to-963800.scan-result.json`、`.scan.log`、`.import.log`、`.replay.log`、`.compare.log`；
- `input.json`、`import-result.json`、`state/assumeutxo-source.json`、`replay-result.json`、`comparison-result.json`。

本次归档引用的关键报告文件SHA-256：

| 文件 | SHA-256 |
| --- | --- |
| replay-result.json | `950e3493e211ada68c90d60a7ded5f4b8443064cb9cced311a4953b9e5116468` |
| comparison-result.json | `69fcfdd4a3e3a25f64adac1bd4ed89228e4955c782d1f3f629a33aa6b5327388` |
| 同级compare.log | `95983de1e7310c27d5be89439a7eb6087f0694800d84bab74ec45ed35080ea7d` |

## 6. 后续工作

P4已证明：恢复963800正确余额和后续重放所需状态，不必让balance-history从创世处理旧区块，
也不必从我们的core/registry大快照导入UTXO或余额。**现阶段工具仍读取旧core作为USDB锚点来源和逐块/终态参考。**

下一步P5应验收点查/历史/聚合查询下界、基线delta语义、registry覆盖声明、state-ref及更多reorg/恢复场景。
P6/P7再处理独立小锚点的发布与信任、摆脱全量txindex就绪依赖、真正独立的新节点冷启动及镜像/安装器集成。
Bitcoin保持 `prune=0` 和官方后台验证；本次结果不意味着这些部署工作已完成。
