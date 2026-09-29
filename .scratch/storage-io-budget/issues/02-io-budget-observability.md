# 02: 存储水位读数 + 慢语句伴随告警层

**What to build:** 决策 321 的可观测两件：
① core 新增 `storage::io_budget` 模块——`wal_bytes` / `db_bytes` / `disk_free_bytes`
（statvfs）三个读数 + `CheckpointOutcome`，**读数失败一律 `None` 不 panic**；
② app 层 `SlowStatementLayer`（`crates/app/src/io_budget.rs`）：订阅 `sqlx::query`
事件，对超过 1s 的慢语句追加一行 `storage::io_budget` 的 WARN 伴随告警，把
WAL 字节数与磁盘剩余钉在同一份现场里（20s 量子复发时日志直接回答「是不是
磁盘 / WAL」）。

**Blocked by:** None

**Status:** done（已实现，决策 321；L2 3 条 + app 单测 2 条全绿）

- [x] `storage::io_budget`：`wal_bytes`（`<db>-wal` 同拼法）、`file_bytes`、
      `disk_free_bytes`（`#[cfg(unix)]` statvfs；字段类型按平台漂移，函数级
      targeted allow）、`CheckpointOutcome` 结构
- [x] `SlowStatementLayer`：target 只听 `sqlx::query`（自己的事件 target 是
      `storage::io_budget`，闸死防递归）；阈值 1s（与 sqlx slow_threshold 同）；
      读数缺失时 `*_present=false` 口供、告警不缺席
- [x] 注册顺序：三种日志格式（json / compact / pretty）下**输出层在前、观察层
      最后**——分发按注册顺序，伴随行必须排在它所依附的慢语句行之后
- [x] 单测：target 闸门（含防递归支）；端到端——伴随行出去且在原行之后、
      `wal_bytes=8` / `disk_free_present=true` / summary / elapsed 都在、
      0.02s 快语句不触发（statvfs 只为慢语句付钱）
- [x] 测试与生产同路：用 `set_global_default`（scoped `set_default` 有分发
      重入守卫，嵌套发事件会落到 no-op dispatcher——那是测试假象）
