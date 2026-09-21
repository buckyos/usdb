# 节点进度展示的分层边界

运维使用说明见[状态与同步进度](../handbook/node/status.md)。本页说明修改面板时的代码边界。

```text
collect_node_progress / assumeutxo_node / usdb_minting
                        ↓ 观测 report
NodeProgressHistory / ProgressTiming（连续观察的旧值、耗时与 ETA）
                        ↓ 展示用 report
node_progress_render.render_node_progress（纯函数）
                        ↓ 文本
TerminalProgressDisplay / print_progress_status（终端输出）
```

- **采集和控制**：RPC、Docker、配置读取、资源与就绪判断保留在原有模块。渲染器不参与启动、重启、放行挖矿或 Ord 的决策。
- **观测增强**：连续观察已有的旧值期限、进程变化、阶段切换和 ETA 计算仍由 `NodeProgressHistory` / `ProgressTiming` 处理。渲染器不计时、不缓存上一帧，不把旧高度重新当作实时数据。
- **纯渲染**：`node_progress_render.py` 仅依赖标准库的文本和数据处理。输入为 report 和显式的 `phase`、`width`、`details`、`unicode`，输出为字符串。内部行模型只用于分组、符号、换行和折叠；不得改写 report，也不得反向成为控制逻辑的输入。
- **终端适配**：主脚本读取终端宽度、编码和 TTY 能力，管理备用屏幕及 Ctrl+C 恢复。渲染器不读取环境变量、不写 stdout/stderr、不发出终端控制序列。重定向和不支持 Unicode 的终端选择 ASCII 标记。
- **机器输出**：`status --progress-json` 直接序列化采集 report，跳过文本渲染；文本布局和内部行模型不属于 JSON 协议。

后续调整分组、符号、文案和折叠优先修改渲染模块。如果需要新观测字段，应由对应采集器提供，不在渲染时补发 RPC，也不从诊断文案解析数值来决定就绪。

`tests/test_node_progress_display.py` 覆盖独立导入、无 I/O 渲染、输入不变、分组、窄屏换行、ASCII、详细选项和 JSON 保真；其他观测测试继续覆盖过期状态、快照阶段、后台历史验证和 controller 的语义。`test_prepare_release_node_kit.py` 从实际打包目录启动 CLI，防止遗漏新模块。
