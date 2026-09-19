# 04: `node_overrides` 的读法（回显 + 会抹掉就拒）

**What to build:** ① `read_stage_configs` **回显** `node_overrides_json`；② `config set` 在
「新配置没带 `node_overrides` 而旧配置有」时**拒绝并说明**（报错要说清旧配置里有几个节点覆盖、
要么带上要么先用 `read_stage_configs` 看清再改）（决策 236）。**不做**「整条替换 → 局部合并」。

**为什么是前置**：决策 228 认了「流水线配置」这一类修复，而它的可执行性卡在这一条上——
`config set` 是整条替换，「留空即清成默认」写在工具描述里，而回显里没有 `node_overrides`，
于是「改之前先看」这条既有纪律**执行不了**。2026-09-18 值班长正是因此**主动拒提**配置改动。

**Blocked by:** None

**Status:** done

- [x] `read_stage_configs` 的读数带回 `node_overrides_json`（顺带 `persona_append` / `env_mode`）
- [x] `config set` 的校验点：会丢掉旧 `node_overrides` 就拒，报文给出处置办法
- [x] 用例：回显真的回来（含「本来就有 / 本来就没有」两侧）；拒绝那一条**什么都不落库**
      （旧配置一字未动）；带上 `node_overrides` 时照常写成功
- [x] 既有语义不动：其余字段仍然「留空即清成默认」

**Notes（实现提示）:**
- 只回显防不住手滑（看见了仍可能静默抹掉），只校验则让它继续看不见现状——两条都要。
- 不翻整条替换语义：那要另立一条裁决。
