# 01: 会话数据与迁移——一条长会话拆成可管理的班次，命令日志同时拿到会话归属

**What to build:** 对讲台从「一条无限增长的长会话」变成「一排可以新建、切换、重命名、归档的班次」。
本票只做**数据与端点**：界面在 02、事件在 03。

**迁移 0012**（落地时以当时实际的空号为准）三件事，一张迁移做完：

- 新表 `kanban_foreman_sessions(id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at, last_active_at,
  archived_at)`；
- `kanban_foreman_messages` 加 `session_id TEXT`，**把既有行回填到第一个会话**（标题取首条用户消息的
  前若干字；没有消息时给一个中性标题）；
- `kanban_node_commands` 加 `session_id TEXT` 并把 `task_id` 改为可空。理由是值班长没有 task_id
  （`pipeline/foreman.rs:416-427` 里是 `String::new()`），而空串会被外键校验拒掉——
  `0004_project_level_runs.sql:11` 的注释写明「不采用哨兵值：SQLite 的外键校验会让
  task_id = '' 直接失败（决策：外键不可伪造）」。手法照迁移 0004 给 runs / conversations 做过的那一套：
  重建表 + 可空 task_id + CHECK。**命令的归属列与会话同批做**，因为「值班长的命令记在哪」和「会话」
  是同一件事。

**存储层**（`storage/foreman.rs`、`storage/observability.rs`）：`list_foreman_sessions`（按
`last_active_at` 倒序、上限一条常量、不分页）/ `create` / `rename` / `archive` /
`list_foreman_messages(session_id, limit)` / `foreman_session_totals(session_id)`。

**顺带修正一处假读数**：`foreman_session_totals` 今天对整张表求和（`storage/foreman.rs:148-158`），
页头那句「本次会话 N tok」实为自建库以来的累计值。按会话过滤之后，它第一次名副其实。

**端点**：`GET /foreman/sessions`、`POST /foreman/sessions`、`PATCH /foreman/sessions/{id}`（改名）、
`POST /foreman/sessions/{id}/archive`；`GET /foreman/session` 改为按会话取（`?session=<id>`，缺省取
最近活动的未归档会话，老客户端的读法不至于立刻断掉）。全部落在 `/foreman/` 前缀下，自动继承配对护
（决策 182⑦ 的既有口径）。

**Blocked by:** None（可立即开始）

**Status:** done

- [ ] 迁移 0012 三件事一张作业：新表 / messages 加列并回填 / commands 加列且 task_id 改可空
- [ ] 回填成第一个会话，标题取自首条用户消息（截断规则写成纯函数并单测；无消息时给中性标题）
- [ ] `list_foreman_messages` 与 `foreman_session_totals` 都按会话过滤，**页头读数从累计值变成真会话值**
      （测试：两个会话各说两句，合计互不污染）
- [ ] 会话列表按最近活动倒序、上限一条常量、不分页
- [ ] 归档 = 置 `archived_at`，**不物理删除**；归档不保护消息，照旧吃 `conversation_retention_days`
      （30 天）的年龄清理——归档是「从列表里收起来」，不是永久保存
- [ ] 四个端点 + `GET /foreman/session?session=`；未接线时仍 503（`foreman_unwired`）
- [ ] 值班长的一条命令能落库（`task_id` NULL + `session_id`）并从会话维度读出来；
      `list_commands` 的任务口径不变
- [ ] 测试：在既有数据上打开不炸；跨会话取数与合计隔离；归档后列表不含它而消息仍在

## 交付

本票已落地（2026-09-17）。逐项：

- **迁移 0012**（`crates/core/src/storage/migrations/0012_foreman_sessions.sql`）一张作业三件事：
  新表 `kanban_foreman_sessions` / `kanban_foreman_messages` 加 `session_id` 并回填 /
  `kanban_node_commands` 加 `session_id` 且 `task_id` 改可空（含迁移 0004 同款的
  `CHECK ((task_id IS NOT NULL) <> (session_id IS NOT NULL))`）。
  回填规则与 `storage/foreman.rs::session_title_from` **逐字等价**（TRIM → 前 24 字符 → 超长补省略号），
  两处都写了「改一处要改另一处」的注释；`resumed_from_pending` 那类哨兵值在命令表里也无处可藏
  （`record_start` 把空串归一成 NULL 并给出人话错误）。
- **迁移在既有数据上的行为有真用例**：`crates/core/tests/foreman_sessions_migration.rs`
  （用 sqlx 迁移器只跑到 0011，造老形态的行，再走 `Store::open`）——2 条。为此给 core 的
  `[dev-dependencies]` 加了一行 `sqlx`（造旧库必须绕过 `Store::open`），理由写在 Cargo.toml 里。
- **存储层**：`list_foreman_sessions`（未归档、`last_active_at` 倒序、上限 `FOREMAN_SESSION_LIST_LIMIT`）/
  `get` / `latest` / `create` / `rename` / `archive`（幂等）/ `append_foreman_message`（同事务刷
  `last_active_at`）/ `append_foreman_user_message`（首句命名，判据是「该会话有没有过消息」而不是
  「标题是否等于中性标题」）/ `list_foreman_messages(session_id, limit)` / `foreman_session_totals(session_id)` /
  `list_foreman_commands(session_id)`。
- **假读数修掉**：`foreman_session_totals` 按会话过滤，页头「本次会话 N tok」第一次名副其实。
  契约测试里那条 `assert_ne!` 起初是错的（FakeAgent 每轮 token 相同，两个一班一句的会话合计天然相等），
  改成**第三个没说过话的班次读出来是 0**——那才是「按会话过滤之前必然失败」的判据。
- **四个端点 + `GET /foreman/session?session=`**：全部在 `/foreman/` 前缀下（继承配对护）；
  未接线时七个端点一律 503；空 home 的读端点返回 `session: null` 而**不建行**（建行是写端点的事）。
- **归档**：置 `archived_at`、不删行、不保护消息；重复归档不改第一次的时间戳。
