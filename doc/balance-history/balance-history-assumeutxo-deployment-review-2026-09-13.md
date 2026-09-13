# AssumeUTXO 正式部署流程复核

日期：2026-09-13。发布分发/打包改动已提交为 `8bd2bbd`，本文记录后续 controller 复核及 node1 重建准备。

## 1. 结论和交付范围

原生 bootstrap、可信旧 commit 检查点、渐进 RPC 重放、按需 LocalLoader、31.1 镜像、
签名 UTXO 分发及正式 node-kit 编排已经实现，可以进入现有**测试网 release CI**，生成用于 node1 冷启动验收的安装包。
真实主网数据的完整安装/重建与长期资源观测仍待执行，不能将本地测试通过称为目标机上线验收完成。

目前源码 `docker/networks/testnet-v0/release-bootstrap.json` 已选定 B=935000、G=963800 和自有签名来源。
基础 network.json 保持原形，由 candidate/publish 共用 `release_bundle.py prepare` 生成原生 bundle；
node-kit 不依赖开发机 `/data`，不打包9GB原始 UTXO或私钥。

## 2. installer → setup → doctor → up 复核

| 环节 | 复核结果 |
| --- | --- |
| installer | 下载并核对 manifest/node-kit；原生 overlay、Python helpers、可信检查点和公开签名材料进入 kit，离开源码目录也能加载 |
| setup | 原生 release 固定 `SNAPSHOT_MODE=assumeutxo`，不再询问旧 BH snapshot；生成独立 BH 数据契约、原始 UTXO/activation 目录和 RPC 凭据 |
| doctor | 无预下载 UTXO、无旧 snapshot 安装标记时可以通过配置预检；仍检查固定来源/签名契约、路径、镜像 digest、资源和网络暴露。增加原生下载/导入提示 |
| up / controller | 自动拉取新 digest 镜像，启动 Core 和128MiB下载/导入 observer；文件准备成功且 Core baseline 可用后进入 overlap 并启动 BH/indexer，无须先等 tip/G |
| 资源切换 | bitcoin → overlap → steady，先按总内存预算停机/重建受影响容器；transition journal 支持中断恢复，原生 observer 纳入预算 |
| BH | 从原始 UTXO 导入 staging，按已到达稳定块重放到 G；全量原点校验后封存，再提供正常服务。重启使用持久进度 |
| 下游门禁 | chain/control-plane 要求 Core 前台及 BH/indexer 共识 readiness；旧 snapshot-loader、registry-installer、paired recovery 不进入原生 Compose 图 |
| status / watch | UTXO下载/文件校验/Core导入、BH导入/重放/校验/服务就绪分别展示；Core后台历史验证单列，保持独立于前台 readiness |

本轮修正：

- Core 导入阶段不再沿用下载结束后的100%字节进度；只在下载和文件校验阶段展示对应字节量。
- `load_uncertain` / `load_failed` 及失败 observer 在进度中明确为 BLOCKED / FAILED；RPC 暂不可用显示 STARTING。
- BH 导入显示已导入 UTXO 数；重放按 B…G 计算百分比；原点全量校验新增 `verifying` 阶段，避免长时间停在“重放100%”。
- 80列终端也能看见独立的 Core 后台高度/验证状态，不依赖可能被截断的 detail。
- doctor 明确旧 registry snapshot 不安装、原生 observed-script registry 由 BH 内部维护。

显示数据不参与 readiness 判断；`sealed` journal、100%进度、端口开放均不能代替实际服务 readiness。
后台验证未完成可以与前台 READY 并存，仍需继续运行官方验证流程。

## 3. 本地验证

- 原发布批次：49项 AssumeUTXO Python测试通过；`usdb-util` 61通过、2既有忽略；workspace check/fmt/clippy通过。
- 本轮：16项原生节点测试通过，包含新增的真实 node-kit tar → installer → setup → doctor 路径，
  下载/导入/重放/后台显示和错误状态；Docker/主机服务使用临时夹具，未操作真实节点。
- 现有安装器、node-kit、控制器/节点测试140项，watch观察/显示5项，资源/恢复32项通过。
- Rust `assumeutxo::native_tests` 7项通过，涵盖原生导入、封存、恢复、重组及不同快照基线的承诺一致性；workspace check/fmt/clippy通过。
- 真实 Compose 展开、旧模式回归仍由节点/资源套件覆盖；GitHub远端构建、目标机实际镜像拉取/长任务未在本轮运行。

此前真实发布材料已用于本地 candidate/publish 重建及 node-kit 隔离加载测试；使用的是测试 release revision/digest，
这些本地 kit 不是可发布的正式版本。匿名来源的小型元数据和 byte-range 检查已通过；全文件验证由 publish 和目标节点各自执行。

## 4. 进入 CI 的顺序

1. 提交并同步本次最终 USDB revision；更新 `go-ethereum/scripts/usdb/ci-revisions.json` 的 USDB pin，
   按既有三仓流程固定 SourceDAO revision。复核时本地 Go lock 仍指向 `ef76df48ddba2feb31bb1e622f5760c2f6dcbf82`，尚未包含本次重构。
2. 依[正式 CI 发布手册](../publish/github-ci-image-and-release-publishing.md)创建两仓同名的新 annotated `rN` tag，
   运行 release-build/Fast，重新构建 Core与服务镜像。旧tag和旧digest不自动获得这次修改。
3. 在该tag运行 candidate；检查 manifest 中 `snapshot.status=native`、`bootstrap_mode=assumeutxo`、B/G/hash及发布记录。
   candidate/publish应生成同一个bundle身份，且仅要求新UTXO公开来源可用，不读取旧BH快照下载记录。
4. 按现有受控 publish 流程发布测试网安装包；核实下载资产可匿名取得后，再在 node1 使用唯一的正式安装入口。
5. 按[现场重建清单](./balance-history-assumeutxo-p74-node-reuse-plan.md)完成旧 BH 保存、全新 BTC/服务数据冷启动及固定锚点比较。

本轮没有 push/tag、触发远端CI、发布在线资产或改动 go-ethereum lock；也没有停止或清理 node1。
node1 另发现旧 Core 因 chainstate LevelDB checksum mismatch 重启；故障属于旧节点现场状态，
需要先保存诊断/备份证据，不能用本轮本地通过结果说明该节点已经健康。详情见上述现场清单。
