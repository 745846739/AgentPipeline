# 01: 模型请求落一张请求级表 + 三行日志 + 进诊断包

**What to build:** 一次模型请求一行（`kanban_model_requests`），**请求开始就写、`finished_at IS NULL`
即「在飞」**，结束补全状态 / 用量 / 收字节总量 / 最后一次收字节的时刻；同批补**三行日志**
（节点开始 / 请求派发 / 请求收场）；这张表的读数**必须进 `read_diagnosis`**（决策 231）。

**为什么先做这一票**：它是「归位」与「量速」共同的地基，也是本批唯一动数据模型的一条。
决策 230 的四项判据里前两项（①哪一个 run ②卡在哪一环）没有它就没有落点——2026-09-19 实测里
值班长采到了正确的方法却把 run 27 的活栈记在 run 26 名下，而**栈里没有任何东西写着它是哪个 run 的**。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] 迁移 `0023_model_requests.sql`：归属三态（`run_id` / `session_id` / 都没有）都可空；
      用量与字节列**可空**（NULL = 没量到，0 = 量到零——决策 226③ 的同一条纪律）
- [x] `Store` 五个读法 + 落行 / 收场 / 启动收口（`orphan_inflight_model_requests`）
- [x] `RecordingLlm` 装饰器包在**构造处**（`Executor::new` / `ForemanRunner::new`）——
      五条 LLM 出口（节点工具循环 / 伪阶段 / 项目分析 / 子代理 / 值班长）一次覆盖
- [x] `AgentResponse` 带出 `bytes_received` / `last_byte_at`（生产适配器在流里量）
- [x] future 被丢掉（墙钟超时 / `select!` 收口）时由 Drop 兜底收成 `timeout`，
      **不留假「在飞」**
- [x] 三行日志：节点开始在 `Store::insert_run` / `insert_project_run`（唯一漏斗），
      派发与收场在 `RecordingLlm`
- [x] `read_diagnosis` 新增一节：`inflight`（此刻在飞）+ `recent`（逐条 run_id / 序号 /
      起止 / 用量 / 字节 / 最后一次收字节）
- [x] 用例：9 条 `model_requests::*`（含「在飞可读」「被丢掉不留飞」「失败不记假 0」
      「重启收口」「值班长落班次不落占位阶段」）+ 1 条诊断包 + 1 条三行日志时间线

**Notes（实现提示）:**
- `run_id = 0` 是「没有 run 行」的哨兵（值班长 / 项目分析），入库前归一成 NULL——0 会撞外键。
- 归属读法与命令台账同形：流水线看 `run_id`，值班长看 `session_id`。
- 不派生 bytes/s：目录里没有对那个比值的判据，多一个没人判得了的数只会误导（同决策 235 拒「置信度」）。
