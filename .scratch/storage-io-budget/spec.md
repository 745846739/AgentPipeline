# Spec: 存储 I/O 预算（synchronous / 维护隔离与 checkpoint / 水位可观测）

2026-09-29 grilling 决议（用户 Q1–Q8 全部「同意」）。缘起是同日的慢 SQL 排查
（日志 `~/.agentpipeline/logs/agentpipeline.log`，162 条 sqlx 慢语句 + 274 条慢
acquire，集中在 9-27 / 9-28）。落点：决策 321，票 01–02（本批同 session 实现）。

## 排查结论（诊断底稿）

慢 SQL 的主体**不是查询形状**——全库 28MB、最大单行 3MB、DELETE 走索引、
本机复现实验里 4 线程并发 3MB autocommit 写最大 880ms、F_FULLFSYNC 80×4 次最大
28ms。三形态：

1. **1–9s 竞争带 + 慢 acquire 洪峰**（9-27，100 条 + 272 条）：高频 autocommit
   写（值班长在途行 250ms 整行重写 ≤3MB、5s 心跳、模型请求台账）× 连接池只有
   5 连接；`synchronous=FULL` 下每次 commit 都是一次 F_FULLFSYNC。整份日志零
   `database is locked`——竞争都在 busy_timeout=5s 内消化，纯排队。
2. **精确 20.00s×49 / 40.00s×6**（9-26/9-28，全是写、全部成功、从各自起点计满）：
   SQLite/sqlx 层无机制可解释（锁等待 5s 封顶且会报错、WAL/行大小/索引/调用方
   饥饿均已实验排除），归因为机器级 I/O 停顿量子——**未归因到具体机制**，靠票 02
   的水位读数在复发时补现场。
3. **任意值长尾 326.4s / 187.4s / 49.5s**（9-27 维护 DELETE，rows_affected=0）：
   维护清理与主流程共抢主池，期间慢 acquire 叠到 24s（整池瘫痪）。

附带环境事实：排查期间磁盘被复现实验写到 100% 满（113GB 盘常态剩 ~16Gi），
APFS 近满时写入停顿是经典来源。

## 决议（Q1(c) + Q2 + Q6(c) + Q4(c) + Q5(b)）

- **`synchronous=FULL → NORMAL`**（Q2 拍板接受语义）：WAL 模式下 NORMAL 只在
  checkpoint 时 fsync；代价是掉电最多丢最近几笔已确认事务（应用不崩、库不损坏，
  重跑 tick 补回）——单机单用户开发管线可接受。判 `PRAGMA synchronous` = 1（L2）。
- **重 DELETE 走维护专用连接**：`Store::maintenance_connection()` 每次开一条不进
  主池的独立连接，保留期 DELETE 不再与主流程抢座；连接参数由 `connect_options()`
  唯一出处保证与主池同参。
- **维护收口跑 `PRAGMA wal_checkpoint(TRUNCATE)`**：收缩 WAL + 打一行存储水位；
  `busy=1`（有读者未退让、没收干净）如实上报不报错，下一趟再试。
- **水位可观测**（Q4c）：WAL 字节数 + 库文件字节数 + 磁盘剩余空间三个读数
  （`storage::io_budget`）进两处——维护作业的 checkpoint 行与**慢语句的伴随告警**
  （app 层观察层，阈值与 sqlx slow_threshold 同为 1s，只对慢语句付 statvfs 成本）。
- **250ms 在途刷写节拍不动**（Q6c）：250ms 的主要成本是 FULL fsync，NORMAL 把这笔
  抹掉后整行重写只剩毫秒级开销（复现 29ms）；降频反而牺牲决策 312①「刷新即见」。
  **决策 312 不修订**。

## 明确不做

- **在途行增量写协议**（segments 分行 / append-only，Q1 的 (d)）：先看 ①②③ 落地后
  的实测再定，现在改是过度设计。
- **连接池扩容**：5 是诊断时的观察面，不是本批裁决项；等水位读数给出实据。
- **归因 20s 量子到具体机制**：静态分析与复现实验均已到顶，交给票 02 的现场读数。

## 票

- `issues/01-pragmas-and-maintenance-isolation.md` —— synchronous=NORMAL + 维护
  专用连接 + TRUNCATE checkpoint（core/storage + scheduler）
- `issues/02-io-budget-observability.md` —— 水位读数模块 + 维护日志行 + 慢语句
  伴随告警层（core/storage/io_budget + app/serve）
