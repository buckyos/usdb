# Live Ord Validator Block-Body Payload-Version Upgrade

## 目标

验证 validator block-body schema 从 `payload_version=1.0.0` 的单 pass payload 演进到 `payload_version=1.1.0` 的 candidate-set payload 后，validator 仍能按各自历史 context 回放两代 payload。

## 场景

1. 在高度 `H` 生成 `v1.0` 单 pass payload。
2. 在更高高度 `H+N` 生成 `v1.1` candidate-set payload。
3. 在同一条 canonical chain 上分别回放两代 payload，必须都通过。
4. BTC head 再前进后，两代 payload 仍需继续通过。

## 期望

- `v1.0` 和 `v1.1` payload 可以在同一升级窗口并存。
- validator 会按 `payload_version` 选择对应的历史校验路径。
- head 前进不会污染任一代 payload 的历史回放。
