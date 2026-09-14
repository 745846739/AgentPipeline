# 03: 配置落地 + 文档 + 测试锚点

**What to build:** 把技能机制实际配到 architect-design 上，并把决策 170 写进本仓的权威文档
（决策日志 / §10.6 / 词汇表 / 测试设计）。

**Blocked by:** 02（需要节点级技能注入生效）

**Status:** done（2026-09-14，决策 170）

- [x] 配置落地：`architect-design` 的 `node_overrides_json` =
      `{"validate_input":{"skills":["grilling"]},"execute":{"skills":["to-spec"]}}`
      （直接写库；`PUT /stage-configs/architect-design` 是等价入口，`DELETE` 可回滚）
- [x] 真应用验证：`serve` 启动通过（证明 `validate_startup` 接受该行）、
      `GET /stage-configs` 回读正确、非法技能名经 `PUT` 返回 400 且不落库
- [x] `docs/decisions.md`：新增**决策 170**，并在决策 47 / 20 行内标注「已由决策 170 修订」；
      头部范围与修订清单同步
- [x] `docs/agents.md` §10.6.2：技能三类来源表 + 正文注入 + 节点级技能说明；
      §10.6.3 `StageAgentConfig` 补 `node_overrides[node].skills` 与其存在理由；
      §10.6.4 合并表 skills 行改为三段并集 + 正文非空校验；
      §10.6.5 补节点级技能配置示例（含 `PUT` 入口与回滚）
- [x] `docs/agents.md` §10.4 与 §6 伪代码的 prompt 段落顺序对齐为
      `[基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]`（原缺 `[工作目录]` 与 `[技能清单]`）
- [x] `docs/glossary.md`：补 **技能（skill）** 与 **节点级技能** 词条（此前完全缺失）
- [x] `docs/testing.md`：§5 prompt 组装行补技能正文与节点级注入锚点；
      §6 配置 fail fast 行补正文为空与节点级定位；决策↔测试映射表补 170 行
- [x] L3 契约：`stage_config_accepts_node_scoped_skills_and_rejects_unknown`
      （往返 + 未知技能 400 + 断言不被拒绝的写入污染）+ `stage_config_rejects_empty_knowledge_skill_body`

**Notes（实现结论）:**
- **编号让路**：并行会话（默认端口 8787→8788）原拟用 170，见本会话已在代码中落 12 处
  「决策 170」引用后**主动让路取 171**，并在决策 171 行内注明。故本批次沿用 170，两条决策
  在日志中按编号顺序相邻排列。
- **未配 `provider_id`**：写入的 `architect-design` 行只有 `node_overrides_json`，provider 走全局
  默认（`resolve_provider_id` 的第四级）。这是有意的——用户可为该阶段单独换模型时再补。
- 用户目录 `/Users/lazyking/.agentpipeline/skills/` 尚不存在，属正常：内嵌技能开箱可用，
  该目录只在用户要覆盖正文时才需要（`Home::ensure_dirs` 已负责创建）。
