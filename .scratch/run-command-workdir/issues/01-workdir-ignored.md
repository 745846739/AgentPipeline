# 01: run_command 的 workdir 参数未生效（始终落在仓库根）

**What to build:** 执行体（develop.execute attempt 6，2026-09-28）自己发现：
`run_command` 声明的 `workdir` 参数没有生效，命令始终落在仓库根——执行体只能改用显式
`cd` 绕过。定位 `run_command` 的参数解析 / 进程 spawn 处，让 workdir 按声明生效
（含不存在路径的报错文案），或把参数从工具声明里摘掉（二选一，以「声明即契约」为准）。

**Blocked by:** None

**Status:** done（2026-10-01 复核后关闭：票面所述 workdir 参数已不复存在，现声明的 cwd 参数真正生效，声明即契约成立——证据链见文末）

- [ ] 复现：agent 调 `run_command` 带 workdir，`pwd` 输出 = workdir
- [ ] 回归：既有不带 workdir 的调用（默认仓库根）语义不变
- [ ] 工具声明与实现一致（声明即契约——参数在 `*_TOOL_SPECS` 里的描述同步核对）

## Comments

- **2026-10-01 收口（复核后关闭）**：票面描述的 `workdir` 参数已不存在——现行声明是 `cwd` 且真正生效。证据链：声明 `crates/core/src/agent/catalog.rs:84`（`cwd`，缺省工作区根）→ 解析 `crates/core/src/agent/tools.rs:1846`（显式 `cwd` → `default_cwd` → worktree_path 三级回退）→ spawn `crates/core/src/exec.rs:459` → `crates/core/src/process.rs:72`（`cmd.current_dir(cwd)`）。集成侧 `command_funnel.rs` 以任务 worktree 为 cwd 走全链。原三项验收按现口径成立：带 cwd 的调用落在 cwd（spawn 处由构造保证）、不带 cwd 缺省工作区（回退链末位）、工具声明与实现一致（catalog.rs 单测钉住参数文本）。
