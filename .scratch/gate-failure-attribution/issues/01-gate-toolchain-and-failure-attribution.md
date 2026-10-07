# 01: 闸门钉工具链 + 环境类失败归因 + pending 携带成因

**What to build:** 从用户视角：闸门因为**环境**（工具链/链接）失败时，任务**不在
merge↔test 之间空转**（那是个确定性死环：test 侧只读 agent 自报、永远报绿），而是
**一次就停在带原因的 pending 上**，看板上直接看得见「闸门的 cargo 是 1.92.0，
而构建缓存是 1.98.0 编的」这类读数；确属用例失败的才走决策 85 的复检。同时 106 部署
侧不再让裸 `cargo` 落到系统那套。

**为什么**：根因链三层取证见 [README](../README.md)。核心是「失败是什么」这件事在
系统里被压成了一个退出码：环境失败与用例失败同形（都是 101）、同归 `Test`、同烧一个
计数、同走 test 复检，而成因**从头到尾没有任何字段承载**——于是既修不了也看不见。
本票把「失败到底是什么」钉进四处：部署（环境对）、闸门（预检 + 归因）、路由（改道）、
pending（携带成因）。

**形状**：

1. **部署侧钉工具链（106 unit，立即）**：`agent-pipeline.service` 补
   `Environment=PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin`
   与 `Environment=RUSTUP_TOOLCHAIN=1.98.0`，与 `Makefile:58/67` 同源、同优先级。
   改后 `daemon-reload` + `restart`。**两条都要**：只前置 PATH 时，rustup shim 按
   **当前目录**找 `rust-toolchain.toml`，而 cargo 编 `~/.cargo/registry/src/...` 里
   自带 toolchain 文件的 crate（决策 175 实证：atoi 钉 1.57）会当场切走；`RUSTUP_TOOLCHAIN`
   优先级高于目录文件，是决策 175 认定的**唯一真钉法**。
   unit 是部署物不是仓内文件——本项目的成形做法是把这类「服务器上怎么起」的读数记进
   `docs/operations.md`（§12.18 一带），本票照办。

2. **闸门环境预检（`executor.rs::run_code_gate` 起跑前）**：读项目根的
   `rust-toolchain.toml` 的 `[toolchain].channel`，与闸门实际会用到的那套
   （`cargo --version` / `rustc -vV`，**经 `run_system_command` 同一环境路径取**）
   比对；不一致 → 直接 `GateFailureKind::Environment` 失败，**不跑测试命令**，读数
   （声明版本 vs 实际版本、`which cargo` 解析结果、`CARGO_TARGET_DIR`）进失败输出。
   这一层是为**下一个没配好的部署**留的：106 这次是配置漂移，下次可能是换机、换
   drop-in、换镜像——预检把「静默用错 rustc」变成一个说得清的失败。

3. **`GateFailureKind::Environment` 与改道**（`types.rs` 加变体，向后兼容：存量 DB
   只有 Lint/Test）：
   - `route_merge` ④ 的确定性分流加它（照决策 391 给 `EmptyBranch` 的先例）——
     **不进 test 复检**，直接打回 `develop.execute`（环境失败该由人处置，打回 develop
     是为把成因注入下一轮 prompt，与空分支同形）。
   - `route_code_gate` 的 develop 分支同理：环境失败也走确定性打回。
   - 决策 391 已把决策 139「确定性失败不绕 test」的判据扩到空分支；本票把它再扩到
     环境类失败，并显式标注**再次修订决策 85 的适用面**（收窄为「真正的用例争议」）。

4. **pending 携带成因（根修可见性）**。两处：
   - `PendingContext` 加一组字段承载「门失败类待办」的成因（`gate_failure_kind`、
     `gate_failures` 计数、闸门命令、日志路径、输出摘要——摘要截断照
     `truncate_gate_log` 的纪律，**不静默丢内容**）。
   - `apply_edge`（`executor.rs:1490`）的路由侧 pending 现在只有 `reason_override`
     一条成因通路；把「按 `PendingKind` + 阶段现取成因」补上，使 `route_merge` /
     `route_code_gate` 的耗尽分支不再是**静态文案**。前端**无需改动**：
     `PendingDossier.svelte:98` 已有 `context.diagnostic` 的渲染路径。
   - 纪律：成因字段是**读数**不是建议；模型看到的仍是闸门日志段（现状不变）。

5. **计数语义复核（决策 108 重审）**：`gate_failures` 目前由 `upsert_merge_result`
   单调保留（`observability.rs:761`），环境类失败与代码性失败同烧一个 `validate_retry_max`
   预算。本票要求**环境类失败不烧该计数**（与决策 391 对空分支申报「不写闸门失败、
   不烧 gate_failures」同款），并复核「单调保留」这条本身在环境失败在场时是否仍成立
   —— 若成立，至少在收口消息里报出**分类后的构成**（几次数用例失败、几次环境失败），
   别让一个总数冒充「代码改不动」的信号。

6. **不用做的**：不改 `test_command_for` 的映射（`cargo test --quiet` 是项目口径）；
   不把 message 格式、也不把「怎么起服务」塞进核心代码——工具链钉法是部署物，
   预检是代码。不用改 `merge.rs` 的空分支改道（决策 391 已落地）。

**验收**：

- 106 上 `GET /tasks/{id}` 的 pending 载体里能看到 `gate_failure_kind` 与读数摘要；
  看板卡片/详情页不需要新控件即可显示。
- 在闸门**原样环境**下把 `PATH`/`RUSTUP_TOOLCHAIN` 抽掉复现 `final link failed: bad value`，
  装回去复跑转绿——两次读数都留痕（这条是「预检真能拦」的反向检查）。
- `route_merge` / `route_code_gate` 的单测补 Environment 分支（含「不进 test 复检」断言）；
  `resume_cause_table_is_the_spec` 与规格表若受影响同批补行（决策 277① 的条数断言）。
- 106 上任务 `01M47RQG4M9533F5TMF1AGJXC8` 解出后走完 merge（**本票的止血部分**已要求做完）。
