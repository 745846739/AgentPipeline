# 02: 回执标签走后端供给——`GET /foreman/tools` + 前端删手抄表

**What to build:** 对讲台回执上四个裸奔的英文工具名变成中文——`read_diagnosis` / `run_readonly` / `repair` / `service` 显示为**读诊断包 / 只读取证 / 修复 / 服务动作**（前两个落词汇表现成词条名，后两个与提议侧现词对齐）。`ForemanToolSpec` 加**必填** `label` 字段：加工具不写标签直接编译不过，漂移在编译期焊死，不需要跨语言比对测试。新端点 `GET /foreman/tools` 出**全量 21 条** `{name, label}`——回执标的是**历史**上的工具调用，不按 `env_mode` 过滤（昨天 `auto` 今天改 `deny`，昨天的回执仍要能翻译），不带 description / schema（前端用不上，interface 能少则少）。前端取数一次缓存 + `labelFor`（认不出 → 原样兜底，清单外的 `spawn_sub_agent` 照旧显示原名——那是有测试的刻意行为），Talk 删掉 18 键手抄 `TOOL_LABELS`（含死键 `delete_file`）。

**Blocked by:** 01（两票都要改 `FOREMAN_TOOL_SPECS` 的 21 个条目与同一段冻结测试，串行防撞；label 并不逻辑依赖 prompt，01 的阻塞是结构冲突）

**Status:** done（2026-09-23）

- [x] `ForemanToolSpec` 加 `label: &'static str` 必填字段，21 条全填；新四词按共识取：`read_diagnosis`→读诊断包、`run_readonly`→只读取证、`repair`→修复、`service`→服务动作，其余 17 条与前端现词逐字一致
- [x] 单测断言**每个 label 非空**（编译器管「有没有」，测试管「是不是空串」）
- [x] 新端点 `GET /foreman/tools` → 按 spec 顺序出全量 21 条 `{name, label}`；不按档位滤、不带 description / parameters
- [x] `api_contract.rs` 契约：21 条、名字与 spec 一一对应且有序、label 均非空；5745 那条 absent-tool（`pairing` / `lan` / `market_repos` 不在清单）测试不动
- [x] 前端 `client.ts` / `types.ts` 加取数函数与类型；lib 小模块 fetch-once 缓存 + `labelFor(tool)`（`?? tool` 兜底语义照搬现表）
- [x] Talk 删掉 18 键 `TOOL_LABELS`，`toolLabel` 与 `receipt` 两处改走 `labelFor`；删表后无残留引用（含 `talk.spec.ts:526` 注释里对 `TOOL_LABELS` 的提及一并更新）
- [x] e2e（真后端 harness）：既有 `读任务台账` / `写文件` 两条文案断言**不改仍绿**；新增一条断言证明四词之一（如「读诊断包」）上了回执；清单外的 `spawn_sub_agent` 回执仍显示原名（兜底没被误伤）
- [x] `make check` 绿
