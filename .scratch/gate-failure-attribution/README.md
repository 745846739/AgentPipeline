# gate-failure-attribution：闸门失败的环境归因与可见性缺口

## 事故实证（2026-10-06 ~ 10-07，任务 01M47RQG4M9533F5TMF1AGJXC8，106）

同一个任务在 commit-contract 票（决策 391）之后**再次**卡死在 merge，形态不同、
根因不同。两次的区分点：决策 391 那次是 `files_changed == 0`（空分支），闸门命令
**根本没执行**；这一次 diff 是实的（12 文件 +1562/−8），闸门**真的跑了**，炸在链接期。

### 读数（106 实取）

| 读数 | 值 | 出处 |
|---|---|---|
| 游标 | `merge/execute`，`status=pending`，`updated_at=2026-10-06T17:38:03Z` | `kanban_node_cursors` |
| pending | `{"type":"retry_exhausted",...,"message":"重试耗尽，需要用户介入"}`（**无 `context` 键**） | 同上 `pending_reason_json` |
| merge 结果 | `gate=fail`、`gate_failure_kind=test`、`gate_failures=3`、approval=none | `kanban_stage_outputs` |
| 闸门读数 | 退出码 101，`linking with cc failed`、`final link failed: bad value`、数百条 `undefined reference to ...llvm.<hash>` | `tasks/<id>/gate-output-merge.log`（18KB） |
| 下发动作 | `{goto merge.execute「重试合并」, cancel}` | `GET /tasks/{id}` |
| 任务本身 | worktree 干净、分支 6 个提交、diff 12 文件 | worktree 实查 |

### 根因链（三层，各缺一块）

1. **闸门命令没钉工具链（环境缺口）**。systemd unit 不设 PATH，服务实跑进程的 PATH 是
   `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin`——**没有 `/root/.cargo/bin`**
   （实取 `/proc/<pid>/environ`）。闸门经 `run_system_command`
   （`crates/core/src/pipeline/executor.rs:1745`）只注入 `CARGO_TARGET_DIR`，于是
   `cargo` 解析到 `/usr/bin/cargo` = **Red Hat rustc 1.92.0**；而共享构建目录
   `{home}/shared-target` 的产物是 rustup **1.98.0** 编的（Makefile:58/67 用
   `export PATH := $(HOME)/.cargo/bin:$(PATH)` + `export RUSTUP_TOOLCHAIN := 1.98.0`
   钉的那套）。两套 rustc 往同一个 target 目录写 → 链接期符号 `.llvm.<hash>` 对不上。
   链接报错里直接带 `/builddir/build/BUILD/rustc-1.92.0-src/...` 的 1.92 std 路径，
   是闸门当时用的就是 1.92 的直接证据。**决策 175 的坑换了形状回来**：Mac 侧是
   MacPorts 插队，服务器侧是 system cargo 插队，共同点是「裸 `cargo` 落到了没被钉住的那套」。

   间歇性（08:57、11:27 的 develop 闸门真跑过 `cargo test` 且通过；17:10 之后持续失败），
   与 test 阶段两次 90 分钟节点超时（14:00、15:30）期间构建被打断在时间上相邻。
   修复验证**不能只验一次绿灯**。

2. **失败被归错类（归因缺口）**。`run_code_gate` 把链接失败归成
   `GateFailureKind::Test`（`executor.rs:1906` 只看退出码），`route_merge` ④
   （`pipeline/routes.rs:231`）据此把它踢回 `test.execute` 复检（决策 85）。但 test 的
   validate_output 是 `test_code_gate`（`executor.rs:984`）——**它一条命令都不跑**，
   只读 test agent 自报的 `test_result` 元数据；agent 用 `make`（工具链钉对）跑是绿的，
   于是自报绿、放行、回 merge、确定性闸门再炸一次。**确定性闸门与环境自报对
   「用哪套 rustc」的判断不一致，这个环注定转不出去**——正是决策 139「确定性失败
   不绕 test」要防的形态，只是这次判据不是空分支而是环境。

3. **成因到不了界面（可见性缺口）**。路由产出的 pending 在
   `executor.rs:1490` 组装消息：只有边带 `reason_override` 时才用真实原因，否则回落
   `pending_message(kind)`（`executor.rs:1996`）那张静态表。`route_merge` 的耗尽分支
   返回 `EdgeKind::Pending(RetryExhausted)` **不带 override**，故落库消息是写死的
   「重试耗尽，需要用户介入」，`context` 整个缺失。前端只有 `reason.message` 一处自由
   文本（`PendingDossier.svelte:88`），`context.kind`/`context.diagnostic` 有才渲染——
   **没有东西可显示**。而证据其实都在：完整链接错误在
   `gate-output-merge.log`，摘要在 `merge_result.gate_failure_output`；只是闸门日志
   仅在拼**下一轮 prompt** 时被读（`model_request.rs:583`），`gate_failure_output`
   在非审批态 pending 上没有任何出口。

### 附带发现：阈值被历史失败吃掉

`gate_failures` 由 `upsert_merge_result` **保留**（`storage/observability.rs:761`，决策 108），
只增不减。三次失败里至少两次（17:10、17:38）是环境性的、与代码无关，却和代码性失败
同用一个 `validate_retry_max = 3` 的预算，把「代码改了三轮还不行」的收口信号污染了。

## 立即止血（2026-10-07，本票落地前）

106 unit 补 `PATH` 前置 `~/.cargo/bin` + `RUSTUP_TOOLCHAIN=1.98.0`（与 Makefile 同源，
**改后须 `daemon-reload` + `restart`**），随后按闸门的**原样环境**在任务 worktree 里
复跑闸门命令验证链接转绿，再 resume 该任务的 merge 游标。取证记在下方「止血实证」。

## 票

- [01-gate-toolchain-and-failure-attribution](issues/01-gate-toolchain-and-failure-attribution.md)
  ——单票收口：部署侧钉工具链 + 闸门环境预检 + 环境类失败改道 + pending 携带成因 +
  计数语义复核。

### 落地状态

**未落地**（2026-10-07 开票）。决策号预留 **392**，落地时续写 `docs/decisions.md`。

### 止血实证（2026-10-07 执行）

unit 改动（备份 `/root/agent-pipeline.service.bak-20261007-toolchain`，`daemon-reload` +
`restart`，服务 active、监听 `0.0.0.0:3389`）：

| 读数 | 改前 | 改后 |
|---|---|---|
| 服务进程 `PATH` | `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin`（实取 `/proc/<pid>/environ`） | `/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin` |
| 该 PATH 下 `cargo --version` | `cargo 1.92.0 (344c4567c 2025-10-21) (Red Hat 1.92.0-1.el9)` | `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| 该 PATH 下 `rustc --version` | （1.92 系列） | `rustc 1.98.0 (88d9e12ae 2026-08-18)` |

闸门命令按**原样环境**（同一 worktree、`PATH`/`RUSTUP_TOOLCHAIN`/`CARGO_TARGET_DIR` 逐字
同闸门）复跑：

| 命令 | 读数 |
|---|---|
| `cargo clippy --all-targets -- -D warnings` | `LINT_EXIT=0`，3m25s（冷缓存） |
| `cargo test --quiet` | `TEST_EXIT=0`（冷缓存 ~12.5min） |
| 两条连跑复跑（暖缓存） | 合计 **1m17s**（远低于 `test_command_timeout_sec=600`） |

resume：`POST /tasks/<id>/resume {"action":"goto","cursor_id":"01M47V86T1CA4586VNCET36RB6",
"target_stage":"merge","target_node":"execute"}` → `{"ok":true,"spawned":true}`。结果：

| 读数 | 改前 | 改后 |
|---|---|---|
| merge 结果 | `gate=fail`、`gate_failure_kind=test`、`gate_failures=3` | `gate=pass`、`gate_failure_kind=None`、approval=pending |
| 闸门日志 | 18521 字节，`final link failed` 命中 | 5855 字节，757 条用例全绿，命中 **0** |
| 游标 pending | `retry_exhausted`（消息写死、无 `context`） | `merge_approval`，下发 `{approve 合入, return 返回修改}` |

顺带清出的存量：`{home}/shared-target` 13G（盘 97%）→ 增量树删除后 5.6G（盘 **73%**，
11G 空闲）。**合入与否是产品决定，未代按。**

**注意**：`gate_failures` 仍是 3（`upsert_merge_result` 单调保留，决策 108）——本票的
闸门通过走的是审批分支、不经过耗尽分支，故不影响本次收口；但这条计数语义正是
形状 5 要复核的对象。
