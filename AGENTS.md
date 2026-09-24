# AGENTS.md

AgentPipeline：kanban 式流水线驱动的本地多 agent 开发管线（init → architect-design → develop-design / test-design 并行 → sync-check → develop → review → test → merge → done）。

设计文档入口见 [docs/README.md](docs/README.md)（含章节编号 §N ↔ 文件对照表）；术语表 [docs/glossary.md](docs/glossary.md)；决策日志 [docs/decisions.md](docs/decisions.md)（#1–261，只追加）。改代码前先读 [docs/testing.md](docs/testing.md) 的用例目录与五条可测试性接缝（决策 143，由决策 177 修订为五条，再由决策 194 修订**形状**——条数仍是五条，第五条从「网络出口加一条 `MarketClient`」改为「仓访问加一条 `SkillRepo`」，再由决策 250 删掉那条只有一处 `impl` 的假 trait、校正为「仓访问的**远端地址替换点**」）。界面层的可追溯性走 [design/frontend-design.md](design/frontend-design.md) §12.3 的「行为 / 规则 → 实现位置」表（决策 199，悬空引用由 `frontend/src/lib/behavior-map.test.ts` 拦下）。

## Agent skills

### Issue tracker

本地 markdown：票以文件形式存放于 `.scratch/<feature-slug>/`，一票一文件（现有 `agentpipeline-v1` 共 22 张）。See `docs/agents/issue-tracker.md`.

### Triage labels

默认五个角色标签：`needs-triage` / `needs-info` / `ready-for-agent` / `ready-for-human` / `wontfix`。See `docs/agentstriage-labels.md`.

### Domain docs

单上下文：词汇表用 `docs/glossary.md`，`docs/decisions.md` 承担 ADR 职责（引用格式「决策 N」，与决策冲突必须显式标注编号）。See `docs/agents/domain.md`.
