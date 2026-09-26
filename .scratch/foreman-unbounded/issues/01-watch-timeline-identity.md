# 01: 值守轮的身份与独立时间线

**What to build:** 值守轮有自己的班次身份；它写的话不再落进人的对话时间线。形态是**同一套表加一个类型列**（不另起一套表）——列表、消息、提议、SSE 的 `session_id` 路由、归档全都按 session_id 组织，加一列即可全部复用；新建表要把这些路径整份抄一遍。

**Blocked by:** None（可立即开始）

**Status:** done（已实现，决策 286）

- [x] 迁移 `0030_foreman_session_kind.sql`：`kanban_foreman_sessions` 加 `kind TEXT NOT NULL DEFAULT 'talk'`（值 `talk` / `watch`）；存量行因此全落 `talk`（配合裁决 12：不回填）
- [x] `crates/core/src/storage/foreman.rs`：`list_foreman_sessions`(:244)、`latest_foreman_session`(:271)、`create`(:283) 接受/返回类型
- [x] `ForemanRunner::resolve_session`(foreman.rs:2283-2304) 拆成两条：人的班次（现状语义）与值守班次（不存在就建一个，固定标题如「值守台账」）；`say()` 仍落人的班次，`watch()`(:2028) 落值守班次
- [x] 路由：`session_wire`/`session_payload`(routes/foreman.rs:1064-1126) 带出 `kind`；`GET /foreman/sessions` 支持按类型取（缺省只回人的班次）
- [x] `kind` / `proactive` 两个派生布尔保留不动——存量行靠它们照旧标对（裁决 12 的另一半）
- [x] core 集成：值守播报落值守班次、人的班次读不到它；迁移测试：存量库升级后人的班次列表与消息归属一个字节不变
- [x] 四门 + 决策落号

## Comments

- 来源：拷问 Q2（数据分家）、Q12（只读账）与裁决 12（存量不回填）。裁决 12 若被推翻（要求历史也分家），本票要多一条回填脚本：把存量里以 `【值守播报】`/`【值守没跑起来】` 开头的行迁到值守班次——那会动已归档会话的归属，代价与本票其余部分同量级。
