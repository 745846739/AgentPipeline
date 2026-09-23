# 01: 删 `pipeline/graph.rs` 与 petgraph 依赖，回填七处，落决策 248

**What to build:** 拓扑的第二份描述退场。删 `crates/core/src/pipeline/graph.rs` 与
`mod.rs` 里的两处引用（`pub mod graph;` / `pub use graph::{…}`），从 workspace 与
`crates/core` 两处 `Cargo.toml` 摘掉 `petgraph`（`Cargo.lock` 随行）；七处机制陈述文档
按 README §三 的口径换成「`landing.rs` 落点表 + `routes.rs` 条件边」；`docs/decisions.md`
追加**决策 248**（README §四 三件事），行 5 加修订标注、行 245⑤ 加已执行标注，
顺带删掉 259 行那个断表的空行；`AGENTS.md` 与 `docs/README.md` 的决策计数 247 → 248。
**一条边语义不动**（决策 39 / 80 / 107 与 petgraph 无关）。

**Blocked by:** None

**Status:** done（2026-09-23）

- [x] `graph.rs` 删除；`mod.rs` 两处引用删除；两处 `Cargo.toml` 摘掉 `petgraph`；`Cargo.lock` 收口
- [x] `make check` 绿（lint + test + frontend + e2e）
- [x] 七处机制陈述回填（README §三 表逐行）：`glossary.md:15`、`overview.md:88`、
      `operations.md:728`、`implementation.md:11/50/52/55-75/151`、`docs/README.md:15`、
      `design/frontend-design.md:502`
- [x] 决策 248 追加（三件事：退场事实 + 修订决策 5 + 接 245⑤ 的口）；
      行 5、行 245⑤ 各加标注；259 行空行删除（既存瑕疵，247 批引入）
- [x] `AGENTS.md:5` 与 `docs/README.md:18` 计数 247 → 248
- [x] README 状态行回填

## 验收

- [x] `grep -rn "petgraph" crates/ frontend/ --include='*.rs'` 零命中
- [x] `grep -rn "petgraph" Cargo.toml crates/*/Cargo.toml` 零命中；`Cargo.lock` 里也已清零
- [x] 5 个旧测试的断言在 `landing.rs` / `routes.rs` 的既有用例里各有名有姓地对得上（README §二）
- [x] `docs/` 与 `design/` 里除决策 248 / 5 的修订标注与 `implementation.md:139` 的退场状态注记外，无残留「基于 petgraph 构建」式机制句
