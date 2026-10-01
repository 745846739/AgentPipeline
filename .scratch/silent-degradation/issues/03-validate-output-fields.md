# 03: validate_output 模板字段对齐 + 模板/schema 一致性机械校验

**What to build:** 两件事——把错的字段名改对，并**加一条机械校验让同类错误不可能再溜进来**。

① **字段对齐**。`crates/core/src/agent/templates.rs:108` 的 `ARCH_VO_SYSTEM`
（architect-design.validate_output）指示 agent 提交：

```
- readiness: boolean
- blockers: string[]（不合格时列出不足之处）
```

而它真正要产出的是 `ValidateOutputMetadata`（`crates/core/src/types.rs:810`）：
`passed: bool`（**必填**）+ `blockers: Vec<String>` + `feedback: Option<String>`。
prompt 里的 `readiness` 在 schema 里**根本不存在**，必填的 `passed` 反而没提。
同族的两个更省事：`templates.rs:156`（develop-design.validate_output）与
`:213`（test-design.validate_output）的「## 输出」段**一个字段都不列**，
只写「调用 submit_metadata」——同样漏掉必填的 `passed`。

② **机械校验**。字段名散在 prompt 常量里、schema 在 `types.rs` 里，两者靠人眼同步，
必然再次漂移。加一条测试，把 AgentNodeKind → 结构体的**既有映射**（`submit_metadata_tool::<T>`
那处，`crates/core/src/pipeline/model_request.rs:770-790`）当作唯一口径：
对每个 agent 节点，用 `schemars::schema_for!(T)` 拿到 `properties` 与 `required`，
再扫该节点 `system_template(stage, node)` 的正文，断言：
- 模板里出现的 `submit_metadata` 字段名（`<name>:` 形态）**必须在** `properties` 里；
- 结构体的每个 `required` 字段**必须**在模板正文里被提到。
这条测试这次就会红（`readiness` 不在 properties、`passed` 没被提到），修完转绿。

**Blocked by:** None

**Status:** ready-for-agent（2026-10-01；批次二）

## 落点

- `crates/core/src/agent/templates.rs`：`ARCH_VO_SYSTEM`（`:108`）、`DEV_DESIGN_VO_SYSTEM`（`:156`）、
  `TEST_DESIGN_VO_SYSTEM`（`:213`）的「## 输出」段。
- 顺带核对 execute 侧四个模板的「## submit_metadata 字段」段是否与结构体一一对应
  （`ARCH_EX_SYSTEM:100`、`DEV_DESIGN_EX_SYSTEM:172`、`TEST_DESIGN_EX_SYSTEM:209` 等）。
- 校验测试放在 `crates/core/src/agent/templates.rs` 的 `#[cfg(test)]` 或
  `crates/core/tests/`，复用 `model_request.rs:770-790` 的映射，**不要另建一张表**。

## 验收

- [x] `ARCH_VO_SYSTEM` 写的是 `passed: boolean`（不再出现 `readiness`），并说明
      `feedback` 的用途与 `blockers` 只在不过时填
- [x] `DEV_DESIGN_VO_SYSTEM` / `TEST_DESIGN_VO_SYSTEM` 补出字段清单
- [x] 新增的一致性测试：**先在当前代码上跑红**（证明它有牙齿），修完转绿
- [x] 该测试对"模板提到 schema 里没有的字段"与"必填字段没被提到"两类都能抓
- [x] 既有 prompt 快照测试（`crates/core/src/agent/snapshots/`）更新并检查 diff 只含本票意图的改动
- [ ] `make check` 全绿（本地按决策 331 跑 `make check-lint` + 改动层的窄跑，全量在 CI）

**明确不做**：不改 `ValidateOutputMetadata` 的结构（不改必填集）；不把 prompt 里的字段清单
改成"从 schema 自动渲染"——那是更大的重构，本票只做对齐 + 闸门。

**来源：** `.scratch/silent-degradation/spec.md` 缺陷 3。注意本票**不是**这次失败的原因
（模型写出了 `passed`，是票 01 的截断吃掉的），所以它是潜伏缺陷，不排进批次一。

## 落地记录（2026-10-01）

- **三个 VO 模板**改成 `passed` / `blockers`（不合格时填）/ `feedback`（可选）三行——`readiness`
  从 prompt 里消失，必填的 `passed` 补上（此前 prompt 写的字段在 schema 里根本不存在）。
- **顺带核对了 execute 侧**：`ARCH_EX_SYSTEM` / `DEV_DESIGN_EX_SYSTEM` / `TEST_DESIGN_EX_SYSTEM`
  都漏了必填的 `readiness`，`DEV_EX_SYSTEM` 一个字段清单都没有（`CodeChanges.branch_name` 是必填）
  ——四处一并补齐。
- **映射抽成函数**：`pipeline/model_request.rs` 的 `submit_metadata_tool_for(kind)`（`pub(crate)`），
  原来内联在 `tool_defs` 里的那张 `kind → 类型` 表只此一份，测试直接问它要 schema，
  **不另建表**（票面的要求）。
- **机械校验**（`agent/templates.rs` 的 `#[cfg(test)]`）：
  - `submit_metadata_templates_match_their_json_schema`：12 个 agent 节点逐个过；
  - `check_metadata_template`：判据本体，两个方向都会拦；
  - `the_consistency_checker_rejects_both_drift_directions`：**反向证据**——拿 2026-10-01 的
    真实错法（多写 `readiness` / 漏提 `passed`）构两份坏模板，断言判据都会报错。
    比"先在旧代码上跑红"更耐久：那条只证明过一次，这条每次跑都在证。
  - 字段抽取只认 `- <小写标识符>: …` 形态（`- AC-1:` / `- {测试文件}:` / 全角冒号都不算）。
- **快照**：`crates/core/src/agent/snapshots/` 与 `crates/core/src/pipeline/snapshots/` 的三个
  `.snap` 里都不含模板字段正文（实测 grep `readiness|passed` 无命中），故**无 diff**。
- 顺手修掉一个自己造的坑：给 `DEV_EX_SYSTEM` 写的 `kanban/{task_id}` 撞上
  `templates_only_use_declared_variables`（模板里不许出现未声明占位符），改成文字表述。
