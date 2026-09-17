# 会话续接：改由原因驱动（resume-semantics）

**状态（2026-09-17）：四张票全部落地，代码已动。** 迁移 0013（原因列）与 0014（删开关列）、
`ResumeCause` + `resume_continues` 判定表、两条人为决策走共同实现、两个既有缺陷一并修掉。


裁决（决策 205）：resume 时续不续接上一段对话，**由原因决定**，不再由阶段 / 节点参数决定；
`resume_continuation` 那层配置**整层退场**。「不展示在设置里、在代码中定义好」。

| 票 | 内容 | 依赖 |
|---|---|---|
| [01](issues/01-cause-driven.md) | 原因列 + `ResumeCause` + 判定表 + 两条绕过 flag 的路统一 | — |
| [02](issues/02-config-retirement.md) | `resume_continuation` 从 API / DB / 前端整层退场 | 01 |
| [03](issues/03-defect-continuation-link.md) | 缺陷：`continued_from_run_id` 在干净重试轮也落链（token **少算**） | — |
| [04](issues/04-defect-overflow-escape.md) | 缺陷：`context_overflow` 的 `model_override` 不解除 pending（按了没反应） | — |

**判定表一览**（决策 205）：

- **true**：校验耗尽（格式不是 json）、信息不足被打回、代码有问题被打回（含评审驳回）、
  超时耗尽后人工「重试执行」、judge 分歧、脏工作区、重复风险、**冲突等待自动放行**、
  **依赖失败自动恢复**；
- **false**：`merge_approval` 通过（merge 的 phase A 是提案、phase B 是执行）、`human_review` 通过
  （去向是新节点，本来就没有自己的旧会话）、以及**一切未列出的原因**（兜底 false）；
- **自动重试不续接**（决策 33 不变）：`validate_attempts` 的原地重试、`agent_retry_max` 的干净重试、
  未耗尽的超时——分界一句话：**模型的自动失败重试不给续接，人的介入才给**；
- **服务重启**：跑到一半被打断**根本不成其为 resume**（`requeue_running_tasks` 只翻任务状态，
  游标不动、不置位），今天靠「flag 没被置」隐式成立——票 01 要求把它写实并加测试钉住。

**修订决策 172⑥ / 180**：参数化那条裁决退场，「节点级 > 阶段级 > 关」的分层读法整层作废。
**续接机制本身保留**：`continued_from_run_id` 指针、压缩锚点边界（`carried_len`）、
`take_continuation` 的取数逻辑都不动——退场的只是「谁来决定开不开」这一层。
