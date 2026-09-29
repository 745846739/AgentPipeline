# 01: synchronous=NORMAL + 维护专用连接 + TRUNCATE checkpoint

**What to build:** 决策 321 的写路径三件：
① `Store::open` 的连接选项加 `synchronous = NORMAL`（WAL 下的正确档位，语义代价
见 spec）；② 选项收进 `Store::connect_options()` 唯一出处，新增
`Store::maintenance_connection()`（不进主池的独立连接），小时级维护的重
保留期 DELETE（`purge_expired_conversations`）改走它；③ 维护收口跑
`Store::checkpoint_wal()`（`PRAGMA wal_checkpoint(TRUNCATE)`），busy 如实上报、
不报错，下一趟再试。

**Blocked by:** None

**Status:** done（已实现，决策 321；L2 storage_io_budget 3 条全绿）

- [x] `connect_options()` 唯一出处：journal=WAL、**synchronous=NORMAL**、
      busy_timeout=5s、foreign_keys；主池与维护连接同参
- [x] `maintenance_connection()`：每次新建独立连接，不占主池 5 个座位
- [x] `purge_expired_conversations` 改走维护连接（重 DELETE 是 326s 长尾的主角）
- [x] `checkpoint_wal()`：TRUNCATE + 水位读数（WAL 前/后、库大小、磁盘剩余）落
      `storage::io_budget` target 的 info 行；维护收口再打一行汇总
- [x] L2 `storage_io_budget`：① `PRAGMA synchronous`=1（主池与维护连接各断言一次）
      ② 主池 5 连接占满时维护连接 2s 内可用、重 DELETE 语义照常（池侧 acquire
      300ms 内借不到座——隔离判据） ③ TRUNCATE 后 WAL 归 0、水位读数为真
- [ ] 真机验证：重负载日维护期间慢 acquire 不再随维护作业放大（等下一次复发对照）
