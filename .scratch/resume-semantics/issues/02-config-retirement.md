# 02: `resume_continuation` 整层退场——设置里不再有这个勾选框

**What to build:** 决策 205 把这层配置删掉（决定权归原因表）。退场清单：

- `stage_configs.resume_continuation` 列（迁移 DROP COLUMN；**历史迁移的 checksum 不受影响**——
  与「0009 死表留着」是两回事，那条是表不能删，这里删的是列）
- `node_overrides_json[node].resume_continuation` 的读法：`config.rs` 的 `node_resume_continuation`
  与 `effective_resume_continuation`（`config.rs:527-544`）一并删
- `PUT /stage-configs` 的出入参（`routes/stage_configs.rs:41-66` 的 `PutStageConfig` 与 `put` 的
  candidate 构造）与 `StageConfig` 结构体字段（`types.rs:1108-1112`）
- `storage/catalog.rs` 的行映射（`into_config`）与 upsert 绑定
- 前端：`components/settings/StageConfigForm.svelte:177-183` 的勾选框、`lib/stageConfigs.ts` 的
  `StageConfigDraft.resume_continuation` / `emptyStageConfigDraft` / `draftFromStageConfig` /
  `buildStageConfigPut`（今天那条「只在 true 时发送」的规则随字段一起消失）、`api/types.ts` 的两处
- 文档：`docs/agents.md` §10.6.3 的 `resume_continuation?` 行与 §10.6.4 合并表那行「节点级 > 阶段级 >
  关」

**保留什么**：`take_continuation`（`pipeline/executor.rs:1026-1058`）的三道 AND 条件改为**两道**——
「原因表说 true」+「有可读会话」；`link_run_continuation` / `continued_from_run_id` / 压缩锚点边界
（`carried_len`）/ `metrics::total_tokens` 的去重口径**都不动**。

**Blocked by:** 01（判定表要先在，否则删了配置就没东西决定开不开）

**Status:** done

- [ ] DROP COLUMN 迁移；`PUT /stage-configs` 不再吃这个字段（旧客户端带上它时的行为要选定一种：
      报 400，或按 `deny_unknown_fields` 的口径静默忽略——落地时选一种并写进交付说明）
- [ ] `StageConfigForm` 的勾选框消失；`lib/stageConfigs.ts` 的四处字段逻辑删除，其单测
      （`stageConfigs.test.ts`）同步
- [ ] `docs/agents.md` 两处描述改写为「由原因表决定」
- [ ] 无回归：续接在「信息不足补充后继续」与「评审驳回回开发」两条路上仍生效（测试钉住）
- [ ] 无回归：干净重试与首跑仍是空 messages
- [ ] `node_overrides_json` 的**其他**键（skills、超时）不受影响——它是个通用覆盖表，只摘一个键

## 交付

本票已落地（2026-09-17）。

- **迁移 0014**（`0014_drop_resume_continuation.sql`）：`ALTER TABLE stage_configs DROP COLUMN resume_continuation`。
- **退场清单逐条**：`config.rs` 的 `node_resume_continuation` / `effective_resume_continuation`（删）、
  `types.rs::StageConfig` 的字段（删）、`storage/catalog.rs` 的行映射与 upsert 绑定 / SELECT 列（删）、
  `routes/stage_configs.rs` 的 `PutStageConfig.resume_continuation` 与 candidate 构造（删）、
  前端 `StageConfigForm.svelte` 的勾选框与其 CSS、`lib/stageConfigs.ts` 的四处（字段 / 空草稿 /
  预填 / build 时「只在 true 时下发」那条规则）、`api/types.ts` 的两处。
- **旧客户端带上这个字段时的行为：静默忽略。** `PutStageConfig` 没有 `deny_unknown_fields`
  （serde 的默认姿态），故旧前端多发的那个键被丢掉，不报 400。**选它而不是 400 的理由**：
  发这个字段的只可能是同一个应用里的旧 bundle（前后端同源、一起构建），而它一旦出现就说明
  这是升级瞬间的一次过期请求——为它专门回一条 400 只会让使用者看到一次无意义的报错。
- **保留**：`take_continuation`（三道改成两道，见票 01）、`link_run_continuation` /
  `continued_from_run_id`、压缩锚点边界、`metrics::total_tokens` 的去重口径——一个都没动。
- **无回归**：`info_insufficient`（true）与评审驳回（true）两条路都有测试；
  干净重试与首跑仍是空 messages（两条既有用例改为原因驱动后照旧绿）。
- **`node_overrides_json` 的其他键不受影响**：摘掉的是**读法**（两个函数），不是表里的数据；
  `skills` / 超时那几处逐条未动，`stage_configs.test.ts` 的节点级技能往返用例照旧绿。
- **`docs/agents.md`**：§10.6.3 的 `resume_continuation?` 行与 §10.6.4 的分层表那一行改为
  「由代码里的原因表决定，不是配置项」。
