# 01: e2e 脚手架回灌

**What to build:** `tests/e2e` 的三个场景文件各自复制了一份 `Flow` 流程脚手架，收回到公共模块一处，并补上后续票要用的断言助手——「断言待办挂载的 `context.kind`」与「断言重入 prompt 含某段反馈」。完成后全部既有 E2E 用例经公共脚手架编译运行、用例数不减，后续涉及 pending 语义与反馈注入的票（05 / 07 / 08）不必再各写一套读取代码。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `crash_recovery.rs` / `happy_path.rs` / `join_and_skip.rs` 三处 `Flow` 副本消失，公共模块成为唯一来源
- [x] 新增助手：读取某任务当前 pending 及其 `context.kind`；读取某次重入的 user prompt 文本
- [x] `cargo test -p <e2e 包>` 全绿且用例数与改动前一致
- [x] 公共脚手架带文件头注，说明 FakeAgent 脚本队列按 attempt 投喂的既有注记（见票 19 文末测试基建注记）
