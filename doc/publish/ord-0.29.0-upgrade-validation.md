# 外部 Ord 0.29.0 升级验证

日期：2026-09-20。范围：可选的本机 Ord 服务，以及 USDB world-sim 的默认源码构建版本。
未改变 USDB Indexer 内嵌的 `ord 0.24.2` Rust 解析库、USDB 协议、网络身份或 sibling go-ethereum 的历史 regtest 工具固定版本。

## 来源与构建

- [官方最新 release](https://github.com/ordinals/ord/releases/tag/0.29.0)：0.29.0，发布时间 2026-08-05。
- 固定源码提交：`7e37a3bd3391044b39f5f11f20dfdb8b3764cd0e`。
- 编译：Rust 1.91.0，`cargo auditable build --release --locked --bin ord`，cargo-auditable 0.7.5。
- 固定依赖：[ord-0.29.0.Cargo.lock](../../docker/locks/ord-0.29.0.Cargo.lock)，以官方该提交为起点，
  仅修补 h2/rustls 及必要的兼容传递依赖；详情见[锁文件说明](../../docker/locks/README.md)。
- 本地上述编译通过，二进制返回 `ord 0.29.0`。最终 services 镜像的完整构建、镜像扫描及线上 CI 仍需在发版时执行。

本地验证输入的 SHA-256（用于对应本轮证据，不作为不同构建环境的可复现字节承诺）：

| 输入 | SHA-256 |
| --- | --- |
| 发布 Cargo.lock | `4415cd1c9624cc7fcd44201791923d08a1afbbc5b3210ef06886b58ba34f232c` |
| 本地 auditable Ord 二进制 | `d9bec27bc03e0270561df3f807debeddc84df07cef4947c8fc420a7bb657fd0b` |
| Bitcoin 31.1 二进制 | `986e63b3c8770f08d0059820ad3dd085d1ab9e1bea23946c243f858a06888a08` |

## 已完成验证

使用本地 Bitcoin Core 31.1（RPC version 310100）和上述 auditable Ord 二进制，创建隔离的 regtest
数据和钱包，全部 RPC/HTTP 仅监听 loopback；无连接已有节点、无生产钱包、无主网转账。

| 项目 | 结果 |
| --- | --- |
| Core txindex 完成、Ord 启动追平 | 通过 |
| 使用生产就绪检查逻辑比对准确高度、区块 hash | 通过 |
| Ord 创建临时钱包、收款、commit/reveal 铭文 | 通过 |
| 铭文 JSON、内容和地址索引查询 | 通过 |
| 替换区块后 orphan 铭文移除、再次确认恢复 | 通过 |
| Core 与 Ord 正常停止/重启后索引持久性 | 通过 |
| 旧目录保留、配置备份、停机限制、失败回滚、关闭 Ord 不创建新库 | 自动化回归通过 |

测试结束高度 109。可复现入口：

```bash
python3 tests/ord_release_smoke.py \
  --ord /path/to/ord-0.29.0 \
  --bitcoind /path/to/bitcoin-31.1/bin/bitcoind \
  --report /tmp/ord-compatibility.json
python3 tests/test_ord_release.py
```

`ord-compatibility.yml` 用 release Dockerfile 的 Ord builder / Bitcoin verifier 阶段构建相同输入，
运行此测试并上传报告；`usdb-release-build.yml` 在发布 services 镜像前等待它通过。

## 数据与性能边界

0.23.3 的 Ord index schema 为 30；0.29.0 为 34，后者明确拒绝旧格式。发布激活选择独立的
`datasets/ord/btc-mainnet/ord-0.29.0`，保留旧目录和旧配置备份，不直接转换旧 `index.redb`。
不开启 Ord 的节点不会启动重建。仅 Ord 的派生数据受此升级影响；Bitcoin/BH/USDB 数据不因此重建。

默认资源限额未改变。本次未进行完整 Bitcoin 主网 Ord 重建，因此不能给出主网完成时间、总索引大小或
峰值资源保证；初次/升级索引时仍需预留磁盘空间，并观察 `minting-status` 的依赖及索引进度。
测试中调用钱包 CLI 仅用于检验外部工具兼容性，私有控制台正式签名和交易功能仍保持关闭。
