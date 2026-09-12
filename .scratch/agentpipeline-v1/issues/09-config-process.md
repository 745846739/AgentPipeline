# 09: 配置校验与进程治理

**What to build:** 启动 fail-fast 校验（vendor 白名单、cross_family_judge 无 provider 拒启、skill 存在性、provider 解析链 node_overrides > task 覆盖 > stage > global）与进程治理（SIGINT 两次语义优雅关闭、启动时清理 executor_owner 残留、目录权限检查）。

**Blocked by:** None（can start immediately）

**Status:** done（已实现）

- [x] validate_startup 全规则 + 启动接线（决策 47/103/134/129）
- [x] 第一次 SIGINT 优雅退出、第二次立即退出（决策 54）
- [x] 启动清 executor_owner 残留（决策 127）
- [x] ~/.agentpipeline 权限检查告警（§12.14 / 决策 112）
