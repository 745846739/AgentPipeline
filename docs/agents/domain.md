# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`docs/glossary.md`**: 领域术语表（原附录 A）。输出中命名领域概念时用这里的词，不要漂移到同义词。
- **`docs/decisions.md`**: 已确认设计决策 #1–#171，只追加、修订关系显式标注。它承担 ADR 职责，引用格式为「决策 N」。若你的产出与某条决策冲突，显式指出而不是静默覆盖（例：_与决策 95 冲突，但值得重开，因为…_）。
- **`docs/README.md`**: 文档地图——章节编号 §N ↔ 文件的对照表（如 §4–5 → data-model.md、§10 → agents.md、§11 → implementation.md、§12 → operations.md）。
- `CONTEXT.md` / `docs/adr/` 目前不存在：按规则**静默跳过**，不要催促创建；`domain-modeling` skill 会在术语/决策真正被解决时惰性创建。

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `docs/glossary.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal: either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `domain-modeling`).

## Flag decision conflicts

If your output contradicts an entry in `docs/decisions.md`, surface it explicitly rather than silently overriding (决策编号只增不改，被修订时在条目内标注)：

> _Contradicts 决策 85 (gate 失败跳回 test.execute)，但值得重开，因为…_

引用决策时一律写「决策 N」，与设计文档的引用约定一致（见 docs/README.md）。
