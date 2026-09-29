# 01: run_command 的 workdir 参数未生效（始终落在仓库根）

**What to build:** 执行体（develop.execute attempt 6，2026-09-28）自己发现：
`run_command` 声明的 `workdir` 参数没有生效，命令始终落在仓库根——执行体只能改用显式
`cd` 绕过。定位 `run_command` 的参数解析 / 进程 spawn 处，让 workdir 按声明生效
（含不存在路径的报错文案），或把参数从工具声明里摘掉（二选一，以「声明即契约」为准）。

**Blocked by:** None

**Status:** needs-triage

- [ ] 复现：agent 调 `run_command` 带 workdir，`pwd` 输出 = workdir
- [ ] 回归：既有不带 workdir 的调用（默认仓库根）语义不变
- [ ] 工具声明与实现一致（声明即契约——参数在 `*_TOOL_SPECS` 里的描述同步核对）
