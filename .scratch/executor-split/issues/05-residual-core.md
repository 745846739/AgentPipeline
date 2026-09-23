# 05: 留守核收口 —— 终态核对、词汇表落表、决策回填（决策 249 · 第五片）

**What to build:** 01–04 全部落地后的收口票：核对留守核定形与五片终态，把票用名落进
词汇表，回填决策 249 的「已落地」段与受影响的文档断言。本片**不搬代码**——它是清点、
落表与收尾。

**Blocked by:** 01, 02, 03, 04

**Status: ready-for-agent**

## 一、清点（决策 249 蓝图 vs 终态）

- [x] `executor.rs` 只剩留守核五样：进程级注册表与取消、`run_inner` 编排与派发表、
      join 屏障编排（`advance_join`）、init/done 薄编排、**闸门执行**、SSE 发射
      （清点时逐项确认「没多没少」，行数是结果不是目标——约 1200–1500 行）。
- [x] 外部 6 触点未动：`new` 五参、`try_run`、`project_analysis`、`force_release`、
      `request_cancel`、类型导出（diff 里 app crate 零改动，或改动有单独裁决）。
- [x] 全仓 `&Executor` 参数归零（今天唯一一个 `post_process` 随票 03 消失）。
- [x] 既有测试**一条不删**：executor 集成 57、e2e 40、契约 137——用例名清点，
      断言不放宽（Q3「只做加法」的终验）。
- [x] 每片新增的窄测试清单汇总，回填 `docs/testing.md` §5/§6 用例目录。
- [x] `make check` 全绿（1037 passed 基线不掉 + 本批新增）。

## 二、词汇表落表（domain-modeling）

- [x] 「模型请求组装」「模型调用编排」是**票用名**（决策 249 已注明未入表）——本票
      按实现定稿的真实领域名补 `docs/glossary.md` 词条（含与「伪阶段」「会话续接」
      「prompt 快照」的互指），票名与代码名对齐。
- [x] 若实现中出现评审未预见的新概念（如 `Prepared` / `RunLedger` 的领域说法），
      同批入表——**先有词条，后有引用**。

## 三、文档回填

- [x] 决策 249 行追加「**已落地**」段（票路径 + 关键性质的见证用例，照决策 245 尾注姿态）。
- [x] `docs/implementation.md` / `docs/testing.md` 里点名 executor 内部函数名的
      **断言式措辞**逐处核对（245 批的先例：`advance_cursor` 更名时同步改了两处——
      本批拆完同类措辞必须不再指向已搬走的名字）。
- [x] 与决策 248（`graph.rs` 已删）核对无残留引用；`docs/README.md` 决策计数随 249 更新。

## 验收

本票的验收就是上面三张清单全勾 + `make check` 绿；它不产生新行为，
**任何一条清点不过 → 回对应票补，不在本票顺手修**。

## Comments

- 2026-09-23 收口完成。**清点**：executor.rs 4223 → 1770 行，余下逐项对上留守核——注册表与
  取消（force_release / request_cancel / cancel_signal / try_acquire）、`run_inner` 编排与
  派发表（含 advance_cursor / apply_edge / pend / pending_context 的投影机件）、join 屏障
  （advance_join / compute_sync_decision）、纯代码节点薄编排（init / done / review-diff /
  verdict）与**闸门执行**（run_code_gate / run_system_command 自由函数）、SSE 发射四出口
  （emit_tool_event / emit_node_started / begin_run_with_sse / finish_run_with_sse）；
  「没多没少」：五片的搬什么逐条有落点，review-diff 等纯代码节点本就不在任何一片的搬移面。
- **外部 6 触点**核过：new 五参、try_run、project_analysis（转发、同签名）、force_release、
  request_cancel、`pub use executor::Executor`——app crate 零改动（git status 无 crates/app/src）。
- **`&Executor` 归零**：全仓仅剩 3 处 doc 文字提及（说的正是「归零」这件事），参数 0 个。
- **测试**：一条不删——executor 集成 57、契约 138、e2e 40 全在（集成/契约的 diff 只有 import
  改道与注释订正；6 条片内随迁测试名字原样——model_request 5 + module_overlap_detection 随 03 到 model_invoke）；新增 23 条窄测试已回填 testing.md 决策 249 行。
- **词汇表**：新增 5 词条（模型请求组装 / 模型调用编排 / run 台账 / merge 状态机 /
  prompt 快照），与「伪阶段」「会话续接」互指（domain-modeling 过一遍；票用名即实现定稿名，
  Prepared / OverflowFacts 等类型说法收在词条内不单立）。
- **文档回填**：决策 249 行追加「已落地」段（照 245 尾注姿态）；implementation.md 扫描
  **零命中**（无需改）；testing.md 三处断言式措辞改道（`executor.rs::tool_defs` →
  `model_request.rs`、历史叙述里的 `merge_phase_b` 加随迁指针、`model_context_window` 标
  实现住处）；README/AGENTS 决策计数已被并行会话推到 #1–250（覆盖本票的 249 要求）。
- **注**：工作树另有并行批次（决策 250 的后续票 23 / repo-id parity）的未提交改动
  （market.rs / repo.rs / testkit / frontend / 本票也改的 glossary+testing）——本批提交
  按文件拆分暂存：两份共享文档只暂存本批的行，对方的行留在工作树。
- 2026-09-23 两轴 code-review 已跑（基点 2577ef3，Spec + Standards 并行子代理）。Spec 三条
  已修：随迁测试计数 5 → **6**（`module_overlap_detection` 随 03）、决策 249 原文「构造留在
  executor」与票 03 的张力在已落地段显式收口（01 检测、03 翻译）、契约 137/138 计数差标注
  （137 为立项读数，247 批 +1 后现行 138）。Standards 硬项已修：testing.md §11 的
  `pipeline/executor.rs::tool_defs_rejects…` 改道 `model_request.rs`；judgement-call 项修了
  `project()` 三份逐字副本（收成 executor 的 `project_or_err` 自由函数，三处共用），
  git 锁等待单调→墙钟的语义注记落在票 02 Comments；判为本票明文授权、不改的：闸门/emit
  自由函数与包装层（票 03/04「SSE 留守、经既有函数调用」）、`test_command_for` 落 merge
  （票 04 明文随迁）、片间互 import（派发入口互调是票据结构）。

