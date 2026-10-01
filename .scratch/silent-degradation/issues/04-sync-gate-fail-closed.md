# 04: sync-check 元数据残缺即判不过（fail-closed）

**What to build:** 让 `sync-check` 在**上游元数据残缺时拒绝放行**，而不是真空通过。

现在它读的是**落库的 `stage_outputs.metadata_json`**（`crates/core/src/pipeline/executor.rs:1008-1043`）：
dev readiness 取自 `(DevelopDesign, dev_doc)`、test readiness 与 `test_scenarios[]` 取自
`(TestDesign, test_scenarios)`、`acceptance_criteria[].id` 取自 `(ArchitectDesign, design_doc)`。
本次事故里这三行的 `metadata_json` **全是 `{"readiness":true}`** —— 于是
`test_scenarios` 数组不存在 → 整段 `design_refs` 引用完整性校验被跳过
（`executor.rs:1044-1088`）→ **直接 `Proceed`**。

也就是说：票 01 掏空元数据之后，唯一能发现"场景清单没了"的关静默放行。这是"残缺被当成合法"
的第三次出现（前两次是 `rescue_truncated_json` 接受前缀、0-token 的 timeout run 被记成正常收场），
也是整批修复里**唯一**能把"元数据被掏空"从静默降级变成可见失败的地方。

改法（决议 Q2）：**fail-closed**。`compute_sync_decision` 在放行前，先校验三行元数据的**必填集**
是否齐全（必填判据与票 01 同源：`schemars::schema_for!(T)` 的 `required`，不另写一份）；
缺任意一项 → 不 `Proceed`，落一条带具体缺项名的失败（`design_refs` 校验本来就有"引用悬空"的
报错通道，走同一条），让任务在闸门处停下而不是带着空元数据走到 merge。

**Blocked by:** 01（共享"必填字段"判据；避免同一件事在两处各写一遍口径）

**Status:** ready-for-agent（2026-10-01；批次一，排在票 01 之后）

## 落点

- `crates/core/src/pipeline/executor.rs:1008-1092` `compute_sync_decision`：
  在现有 readiness 判定之前插入"元数据完整性"判定。
- 复用 `crates/core/src/pipeline/routes.rs` 的 `SyncDecision` 表达（`warnings` 之外
  需要一条能表达"缺项"的通道——`dev_blockers` / `test_blockers` 是既有字段，
  优先用它而不是加字段，加了要同步改消费方）。
- 判据来源：`crates/core/src/types.rs` 的 `ArchitectExecuteMetadata` / `DevelopDesignMetadata` /
  `TestDesignMetadata` 的 `required`。

## 验收

- [x] 单测：三行元数据都是 `{"readiness":true}` → `sync-check` **不 `Proceed`**，
      错误/blockers 里能读出缺的是 `test_scenarios` / `acceptance_criteria`
- [x] 单测：元数据齐全 → 照旧 `Proceed`（不许把正常路径判死）
- [x] 单测：元数据齐全但 `test_scenarios` 里 high 场景的 `design_refs` 指向不存在的 AC
      → 照旧拦下（既有行为不许回归）
- [ ] 端到端：用本次事故任务的真实库副本（或等价 fixture）跑一遍 `sync-check`，
      修前 `Proceed` / 修后拦下——**这条是"闸门真的有牙齿"的证据**
      （**本地只做到「等价 fixture」**：事故任务在 106 的库里，本机 `~/.agentpipeline` 没有它。
      见下方落地记录里的 106 命令）
- [ ] `make check` 全绿（本地按决策 331 跑 `make check-lint` + 改动层的窄跑，全量在 CI）

**明确不做**：不在 `sync-check` 里做"元数据与文档正文是否一致"的语义比对（那需要 LLM，
`sync-check` 是纯代码节点）；不顺手回填历史元数据（回填是一次性脚本，见下）。

## 附：回填脚本（本票交付物之一，一次性、不入产品面）

本次事故里 **architect-design 的完整元数据可以从历史会话原样捞回**：
`kanban_node_conversations` id 98 的第 110 条消息正文里有完整 XML，
八个字段齐全（`readiness` / `design_doc_path` / `affected_files` / `new_symbols` /
`conflict_warnings` / `duplicate_risk` / `acceptance_criteria` / `blockers`，2517 字符、完整闭合），
而落库的 `arguments` 只剩 `{"readiness": true, "design_doc_path": `。test-design 同理（id 88，
6567 字符，含完整 `test_scenarios`）；**develop-design 捞不回来**（id 90 只有一条散文消息，无 XML）。

- 形态：一次性脚本（只读 `kanban_node_conversations` + 写 `kanban_stage_outputs.metadata_json`），
  放 `.scratch/silent-degradation/tools/`，**不挂产品入口**——它是事故恢复，不是常规能力；
  产品里挂"重建元数据"的按钮等于给"元数据不可信"发永久通行证。
- 它的第一用途是**给票 01 当回归 fixture 的来源**，其次才是"万一续走再撞墙"的备选。
- 回填与"跳过重跑"无关：`goto` 只接受阶段入口节点（`pipeline/resume.rs:285-290`），
  且上游没有"有产出就跳过"的机制（`executor.rs:424-555`），落到任何点都会顺流重跑到底。
  （这条弯路在 spec.md 的「已撤回的选项」里记着，别重走。）

**来源：** `.scratch/silent-degradation/spec.md` 缺陷 4 与「已撤回的选项」。

## 落地记录（2026-10-01）

### 判据（与票面的一处偏离，先记在这）
票面写的是「必填集按 `schemars::schema_for!(T)` 的 `required`」。**照那写会拦不住这一次**：
`ArchitectExecuteMetadata` / `DevelopDesignMetadata` / `TestDesignMetadata` 三张结构体里，
除 `readiness` 外全是 `#[serde(default)]`（为兼容历史产出），`required` **只有 `readiness`**
——而事故里三行恰恰都有 `readiness`。

所以判据落成「**闸门真正消费的那两个键在不在**」：
- `acceptance_criteria` 不在 → high 场景的 `design_refs` 无处比对；
- `test_scenarios` 不在 → 整段引用完整性校验（决策 136）进不去。
两处都是「缺了会**静默跳过**」的字段，也正是票面验收点名要读出的那两个名字。纯函数
`sync_metadata_gaps(test_skipped, arch_meta, test_meta)`，四个单测盖住（残缺 / 齐全 / 跳过 test 支 /
整行缺失）。

### 通道（第二处偏离）
票面说"优先用既有 `dev_blockers` / `test_blockers`"。**加了 `SyncDecision.metadata_gaps`**：
缺项是**某一行产出**的残缺，不是某一支的 blocker；塞进 `test_blockers` 会让回溯的读号者
去找错分支（那明明是 architect 的行）。`from_sync_decision` 只投影 `passed`，没有别的
字段级消费方（前端按 `sync_decision.json` 整体展示，grep 无字段引用），故加字段是安全的。
它同时进 `backtrack-feedback.md`（决策 126 的同一通道），重跑的设计阶段看得到。

### 判定
`proceed` 加一项 `metadata_gaps.is_empty()`；不 `Proceed` → 既有的 `Backtrack` →
`backtrack_cursors` 落到 `architect-design.validate_input`（`storage/cursors.rs:266`），
整条设计阶段顺流重跑，残缺的元数据会被重新产出——不会卡在"缺项修不了"上。

### 测试
`crates/core/tests/integration/executor.rs`：
`degraded_stage_metadata_blocks_the_sync_gate`（用 `submit_metadata_raw` 造「键不在」，
类型化 `submit` 表达不了——它永远会把 `acceptance_criteria: []` 发出去）、
`intact_metadata_with_a_dangling_ref_still_blocks`（反面：缺项判据不许把既有引用校验挤掉）。
"齐全 → 照旧 Proceed"由既有的 happy path 用例覆盖（`executor_drives_full_happy_path_to_done`
要穿过 sync-check 才到 develop）。

### 回填脚本
`.scratch/silent-degradation/tools/backfill_metadata.py`（一次性、不入产品面）。口径与产品侧的
`metadata.rs::find_xml_tool_call` 逐条对齐（JSON 优先、退回字符串）；只读
`kanban_node_conversations`，只写 `stage_outputs.metadata_json`，默认 dry-run。

**在 106 上的跑法**（等部署后执行，本机没有事故库）：
```
python3 .scratch/silent-degradation/tools/backfill_metadata.py \
    --db ~/.agentpipeline/data/agentpipeline.db --task-id <事故任务 id>          # dry-run 先看
```

### 落地记录：106 上的实跑（2026-10-02 00:34，部署 631b202 之后）

dry-run 命中四条，落在**三个会话**里（architect-design conv 84 / 98，test-design conv 88）——
事故任务在 09-30 到 10-01 之间被重跑了多轮，每轮各留一份可捞的 XML。写库按会话 id 升序，
**最后一份 architect-design（conv 98）覆盖 conv 84**，符合「取最新一轮产出」的直觉。

落库前先做了一件票面没写的事：**比对捞回的 `design_doc_path` / `test_scenarios_path` 与
`stage_outputs.file_path`**，确认不是「把另一份文档的元数据贴到这一行上」——
两者逐字相同（`…/design.md`、`…/test-scenarios.md`），回填是诚实的。

- 备份：`sqlite3 <db> ".backup /root/agentpipeline-db-bak-<ts>.db"`（在线备份，29M）。
- 结果：`architect-design/design_doc` 从 `{"readiness":true}`（18 字节）→ 2317 字节，
  含 `acceptance_criteria` 5 条；`test-design/test_scenarios` → 6455 字节，含 13 条场景。
- **捞不回来的那一行**：`develop-design/dev_doc` 仍是 `{"readiness":true}`——会话里只有散文、
  没有 `<function=submit_metadata>` 的 XML。脚本对它只会报告「没找到」，这是已知边界，
  不是这次漏做。它不影响本票的闸门（闸门只消费 architect 的 `acceptance_criteria` 与
  test 的 `test_scenarios` 两个键）。
