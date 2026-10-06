# 01: 提交契约进模板 + 零分支守卫 + merge 空分支改道

**What to build:** 从用户视角：一个 develop 阶段产出全绿但变更从未落进任务分支的任务，
**在 develop.validate_output 就被打回**（prompt 附「分支零提交 + 工作区 N 处未提交」的
事实与落提交指令），而不是像 2026-10-06 的 01M47RQG4M9533F5TMF1AGJXC8 那样穿越
review/test 在 merge 才炸、再耗一整轮 test 复检加一次人工决策；确属零变更的任务在
develop 处显式申报后由用户确认提前收口，不白跑 review/test，也不强迫造假提交。

**为什么**：根因链四层取证见 [README](../README.md)。核心是提交契约在系统里没有归属：
模板不要求、闸门不校验、merge 炸了还归错类。本票把「归属」钉进四处——模板（要求）、
develop 闸门（校验）、merge（改道）、重入 prompt（自愈）。

**形状**：

1. **模板根修（`crates/core/src/agent/templates.rs`）**：
   - `DEV_EX_SYSTEM` 输出步骤加提交步：先 `cd` 到系统注入的 worktree 绝对路径
     （相对 cwd 会解析到别的 checkout，事故实证），逐块 `git add` + `git commit`，
     message 遵循目标仓提交惯例（**不进核心代码**，message 格式属项目域）；收口前自查
     `rev-list --count <基准>..HEAD` > 0 且 `git status --porcelain` 干净、读数写进正文。
   - `DEV_EX_SYSTEM` / `REVIEW_EX_SYSTEM` / `TEST_EX_SYSTEM` 补「每一轮的最终动作必须是
     调用 submit_metadata；声称已交不等于已调用工具」契约锚点句（决策 277① 的扩展，
     develop attempt 3 事故的直接对症）。
   - golden snapshot（`assembled_system_prompt` 等）与 `prompt_template_hash` 更新属预期。
2. **申报制 schema**：develop.execute 的 `submit_metadata` schema 加 `no_changes: boolean`
   （缺省 false，`model_request.rs::submit_metadata_tool_for`）；模板字段清单同步——
   `submit_metadata_templates_match_their_json_schema` 会拦漂移。
3. **develop 侧守卫（`executor.rs::develop_code_gate`）**：lint+单测之外加零分支检查——
   `git rev-list --count {base_ref}..{branch}` 为 0 且未申报 `no_changes` 且工作区有变更
   → fail 打回 execute（计入 `validate_attempts`），失败输出含 rev-list 读数、
   `git status --porcelain`（截断）与落提交指令。base_ref 取法与 merge 阶段 A 同源
   （有 remote `origin/{default_branch}`，无 remote `{default_branch}`，决策 41）。
4. **零变更提前收尾**：申报了 `no_changes` → develop.validate_output 置
   `pending(user_decision)`；用户确认后任务以 **cancelled** 终态收口
   （不经 done——`do_done` 的 merged 硬校验对零变更无语义）；不确认则打回继续。
5. **merge 空分支改道（`merge.rs:243` / `types.rs::GateFailureKind`）**：
   - 枚举加 `EmptyBranch`（存量 DB 只有 Lint/Test，加变体向后兼容）。
   - 空分支且已申报 `no_changes` → `pending(user_decision)`（确认零变更收尾）。
   - 空分支且未申报 → 照写 `gate=Fail` 但 kind=`EmptyBranch`，**直接打回
     develop.execute**（决策 139「lint 失败不绕 test」同款先例），不走决策 85 的
     test 复检——决策 85 的适用范围就此收窄（见决策 391）。
6. **重入注入段（`model_request.rs` 五追加段机制，`rs:461` 起）**：新增「零提交事实段」，
   照既有「先落库/落文件、重入渲染」形状；内容 = rev-list 读数 + status 列表 + 硬话指令
   （落提交或申报 no_changes），**不带 message 格式**。develop 守卫与 merge 改道两条
   路径共用。
7. **止血撤退**：本票部署后删 106 `stage_configs.develop.persona_append`（README 止血记录）。

**Blocked by:** None（可立即开工；106 止血 persona 已上线，dogfood 期间 develop 会正常提交）

**Status:** ready-for-human（本地落地完成，待部署 + 撤退 persona）

**落地记录（2026-10-06，本地直改）**：形状 1–6 全部落地；决策 391 已续写、testing.md
已补三处表行。覆盖用例名：
`crates/core/tests/integration/executor.rs::{develop_gate_kicks_back_when_changes_are_never_committed,
develop_declared_no_changes_pends_then_cancelled_terminal}`、
`tests/e2e/tests/integration/gates.rs::{e2e_merge_empty_branch_kicks_back_to_develop_without_test,
e2e_merge_declared_no_changes_pends_for_user}`；L1 另见 `routes.rs` / `actions.rs` /
`templates.rs` 各条（testing.md §5「提交契约」行）。**golden 无漂移**：三条 insta 快照
（`system_prompt` / `assembled_system_prompt` / `assembled_user_prompt`）都取 architect 节点，
不受 execute 模板改动影响；`prompt_template_hash` 用例只断言长度与非空，故变化被既有用例接住。
**形状 7（撤退 persona_append）待部署批次执行。**

**code-review 两轴复核（同日）**：standards 轴报 1 硬项（`resume_cause_table_is_the_spec`
仍 24 条未含新变体，定长数组会静默放过）+ 3 处重复；spec 轴报 1 处真缺陷 + 3 处弱断言。
已修：①`merge.rs` 申报分支的 pending **补带 `context.kind=zero_changes`**（原先落到通用兜底行
{skip, cancel}，下发不出 {goto develop, cancel}）；②`declared_no_changes` 提为口径单点
（executor / merge 共用）；③`resume_cause_table_is_the_spec` 补 ZeroChanges 行 + 条数断言；
④删 routes.rs 重复的小节注释；⑤新增「干净工作区 + 零提交也不放行」用例；⑥merge 申报用例改
断言「动作集恰为 {goto, cancel}」与「不写 merge_result」（原断言在 None 上恒真，抓不到上面
那条缺陷）；⑦模板用例补 `cd 绝对路径` / `rev-list` 自查两条必需信息。形状 3 的
「工作区有变更」子条件**有意不设**（比票面宽一档），已在决策 391 ③ 记明理由。

- [x] 用例（钉原缺陷）：develop.execute 全绿但变更未提交 → validate_output fail 打回，
      重入 prompt 含 rev-list 读数与 status 事实段（现在这条是放行——事故的洞）
- [x] 用例（申报放行）：申报 `no_changes` → pending(user_decision)；确认后 cancelled 终态；
      `do_done` 的 merged 校验不拦此路径
- [x] 用例（merge 改道）：空分支未申报 → `gate_failure_kind=EmptyBranch`、直接回
      develop.execute、**不进 test**；gate_failures 照决策 108 累计
- [x] 用例（merge 零变更确认）：空分支已申报 → pending(user_decision)
- [x] 用例（契约锚点）：develop/review/test 三个 execute 模板都含「最终动作必须是调用
      submit_metadata」锚点句（对齐决策 277① 的既有判据形状）
- [x] golden 更新且 `prompt_template_hash` 变化被既有用例接住；模板↔schema 一致性测试过
- [x] 决策日志续写决策 391：显式标注**修订决策 85**（空分支排除出 test 复检）、
      **扩展决策 139**（确定性失败不绕 test 原则落到空分支）、终态语义注记
      （零变更任务 cancelled 收口）；`docs/testing.md` 补表行
- [ ] 形状 7：部署到 106 后删除 `stage_configs.develop.persona_append`（同批撤退）

