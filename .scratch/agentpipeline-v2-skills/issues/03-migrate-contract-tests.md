# 03: 契约迁移——内嵌技能断言改用用户目录技能

**What to build:** 把现存 **50 处**按名字钉住 `grilling` / `to-spec` 的断言，改为对**真实的用户目录
技能**断言（临时 home 内写入 `skills/{name}/SKILL.md`），保留原有的注入行为断言意图。

这是「移除内嵌技能」这个宽重构的 **migrate 阶段**：内嵌技能此时**仍然存在**，因此每一步都保持
CI 全绿——迁移完的测试用用户目录 fixture，未迁移的仍用内嵌，两者并存直到收口。

分布（50 处）：`crates/core/tests/executor.rs` 13、`crates/core/src/agent/skills.rs` 21、
`crates/core/src/config.rs` 7、`crates/core/src/agent/prompts.rs` 6、
`crates/app/tests/api_contract.rs` 3。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `skills.rs` 的 21 处：改为临时 home + `write_skill()` 辅助构造知识型技能，断言三类发现 /
      同名覆盖 / 去重保序 / 两种渲染等**行为**不变（断言对象从内嵌常量换为用户文件）
- [x] `config.rs` 7 处：节点级技能声明的校验用例改用用户目录技能，保留「定位到节点」的报错断言
- [x] `prompts.rs` 6 处：段落顺序、`### {name}` / `- {name}` 两种渲染、`prompt_template_hash`
      对正文敏感的用例改用用户技能
- [x] `tests/executor.rs` 13 处：节点级技能注入（同阶段两节点各含对方没有的正文）、阶段级与节点级
      并集——改用真实用户目录技能，保留「同阶段两节点注入不同正文」的核心断言
- [x] `api_contract.rs` 3 处：`PUT /stage-configs` 的技能字段用例改用用户目录技能
- [x] 迁移后**内嵌技能相关断言的意图一条不少**（用 grep 对照迁移前后断言清单）
- [x] 全仓测试保持通过（`cargo test --workspace`）

**Notes（实现提示）:**
- 不新建测试辅助层：`skills.rs` 的 `write_skill(root, name, content)` 已存在，提升为跨文件可复用的
  fixture 即可（或各文件内联同构的局部辅助）。
- 这一步**不删** `EMBEDDED_SKILLS`；删除是票 04 的事。若在本票里顺手删了，CI 不会有中间绿灯。
