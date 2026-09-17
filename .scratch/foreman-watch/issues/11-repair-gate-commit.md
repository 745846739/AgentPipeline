# 11: 修复的闸门 + diff 交付 + 单独成 commit

**What to build:** 修复必须过闸门才算「改完」，改动单独成 commit，diff 交给值班经理审。

## 11.1 闸门必过

修复必须过闸门（lint + test）才算「改完」；没过就播报失败、**不出 diff**。
闸门是现成的：`test_command_for`（`crates/core/src/pipeline/executor.rs:3584-3591`）已把
cargo / pytest / npm 映射好；本仓的 lint 是 `cargo clippy --all-targets -- -D warnings`。

**「快」在这里的定义是「不用等你来钉它」，不是「跳过验证」。** 跑一遍 `cargo test --quiet` 是分钟级；
没有它，你早上审的是一份**赌注**而不是一份补丁——而这份补丁会被合入主干。

## 11.2 单独成 commit

修复的改动**单独成一个 commit**，commit message 带标记。

目标项目那一类的修复会**当场生效**（改任务 worktree → 跑闸门 → resume → 任务带着它写的代码
继续跑），而你第一次看到这些改动，是在任务走到 merge 时那份**混在一起的 diff** 里——
它手写的代码和 agent 写的代码在 diff 里长得一模一样。

**不做标记的账单会在三个月后到期**：那时你会盯着一行代码想「这是哪个 agent 写的、谁让它这么写的」，
而答案不在库里。带标记的 commit 顺带给了你一条天然的审计线。

## 11.3 diff 交付

出的 diff 与闸门读数一起进票 12 的修复提议。**合入永远人按**——走既有那颗 merge decision 钮
（`POST /tasks/{id}/merge/decision`，`crates/app/src/routes/tasks.rs:629`）或你自己 `git merge`，
**不新增第三条路**。

**Blocked by:** 10

**Status:** ready-for-agent

- [ ] 修复完成的前置条件是闸门通过；未通过 → 播报失败原因（lint 还是 test、哪个用例），**不出 diff**
- [ ] 闸门命令与读数落库（复用 `kanban_node_commands` 的 system 源与 `gate-output-{stage}.log` 同款
      路径约定；修复没有 stage，命名要自洽）
- [ ] 修复的改动**单独成 commit**，message 带可检索的标记（含修复 id 与诊断结论一句话）
- [ ] diff 生成口径与 merge 阶段一致（`{base}..{branch}`，`Git.diff_stat`）
- [ ] 「当场生效」那一路（目标项目）与「等合入」那一路（本仓）**分开实现**：
      本仓**不许热修、不许自己重启**，出 diff + 播报 + 把任务标成「等修复合入」
- [ ] 「等修复合入」的标记落在任务上（新增一个 `PendingKind`，或复用 `pending` + 说明）——
      它让「我在等什么」在**任务本身**上看得见，而不是只在你早上读播报时才知道
- [ ] 新增用例：闸门不过 → 无 diff 产出、有一条失败播报
- [ ] 新增用例：闸门过了 → commit 存在且 message 带标记；diff 的范围是 `{base}..{branch}`
- [ ] 新增用例：本仓的修复路径**不含**任何重启调用、也不改工作区（后者可用「工作区脏」反向断言）

## 备注

**「不试无用的 resume」**：诊断结论若是「流水线自身的 bug」，代码不变 → resume 必然同样卡住。
出 diff + 播报 + 标「等修复合入」，**不吃票 08 的 N 次配额**。一个能撑过 `agent_retry_max = 3`
才失败的东西，重跑一次几乎必然同样失败。
