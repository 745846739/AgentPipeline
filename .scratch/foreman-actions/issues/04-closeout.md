# 04: 收口与文档回填（守卫、§12.3、testing.md）

**What to build:** 搬迁完成后的收口——让「分派器只有一处」与「直接动作面只有一处实现」这两条
有东西守着，并把文档回填到与代码一致。

**Blocked by:** 01、02、03。

**Status:** done（2026-09-23）

- [x] **静态守卫**：一条断言「`crates/app/src/routes/foreman.rs` 里不再有 `Git::` /
      `pipeline::repair::` / `pipeline::unstick::` 的直接调用」——照
      `frontend/src/lib/talkLayout.test.ts` 与 `lib/format.test.ts` 的「读源文本 + 正则」先例
      （`@vitest-environment` 那套是前端；Rust 侧可用一条 `#[test]` 读文件文本）。
      **理由**：单测只能证明新 module 对，证明不了路由层没有又长回一份
- [x] **分派器断言**：一条测试钉住 `run_proposal_tool` 的六条臂都在（工具名清单与
      `FOREMAN_TOOL_SPECS` 里属于这六族的那些逐一对上）——防止将来加一个族时漏接一条
- [x] `docs/testing.md`：§3.1 的接缝表**不加行**（本批不新增接缝——`StewardActionRunner`
      与 `for_foreman` 的先例都是「既有替换点上的槽位」），但 §5/§7 用例目录加 service / repair 两行
- [x] `design/frontend-design.md` §12.3：若提议确认钮那些行为行引用了路由层的实现位置，
      改指 `foreman_actions.rs`
- [x] grep 全仓确认：`routes/foreman.rs` 从约 1307 行降到约 1160 行（**是结果不是目标**，
      照决策 249 的口径不设行数硬指标）
- [x] `make check` 绿

## Comments

- **不新增接缝**：决策 143→177→194→250 一路维持「接缝数不随功能数线性增长」。
  本批只是把既有实现挪到它该在的 crate，没有引入新的替换点，故 §3.1 表**不加行、不改行**。
- **行数是结果**：决策 249 明写「不设行数硬指标（4223 → 约 1200–1500 是结果不是目标）」，
  本票同口径。
