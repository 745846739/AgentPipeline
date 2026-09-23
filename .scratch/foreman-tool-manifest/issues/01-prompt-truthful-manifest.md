# 01: 值班长工具清单收成一个值——prompt 按广告集说真话（决策 247 落档）

**What to build:** 值守轮和 `deny` 档下，值班长的系统提示词**不再广告这一轮已被摘掉的工具」——工具纪律段里列的名字与广告集、执行点白名单是同一份 `available`：`respond_inner` 先算 `deny`，`available` 算**一次**，传给广告集（`tool_defs`）、执行点白名单（`foreman_tooling`）与 `system_prompt` 三处。顺带删掉 `ForemanToolSpec` 上手标的 `ForemanToolLayer` 枚举字段（它在生产代码里唯一的消费者就是 prompt 分组）：「你能直接用 / 会改动东西」两组改从档位谓词派生（`is_env_write_tool ∨ is_service_write_tool` → 会改动东西），档位清单（`ENV_TOOLS` / `ENV_WRITE_TOOLS` / `SERVICE_WRITE_TOOLS`）成为分类的唯一事实源，`gate_decision` 与 prompt 读同一份事实。`foreman.rs` 里「广告集、执行点白名单与人格里的工具纪律段同源」那句注释由本票兑现——从文档声称变成实现事实。`power_discipline` 手写文案不动（列表过滤对了它自动变真）。本票同时把**决策 247** 追加进 `docs/decisions.md`（六条裁决一次记齐：分类唯一事实源、删层枚举、available 算一次传三处、spec 加必填 label、`GET /foreman/tools` 全量不滤、`proposalToolLabel` 收 labels 参数与四个新回执词），后续两票引用它。

**Blocked by:** None（可立即开始）

**Status:** done（2026-09-23）

- [x] `docs/decisions.md` 追加**决策 247**（本批六条裁决；246 已被 `.scratch/host-policy` 预留，本批取 247）
- [x] `respond_inner` 顺序上移：先算 `deny`（值守轮摘 `read_conversation` / `run_command`，人的轮不摘），`available = foreman_available_tools_except(mode, deny)` 算一次，传给 `tool_defs`、`foreman_tooling` 执行点白名单、`system_prompt` 三处——三处不再各自取数
- [x] 删 `ForemanToolLayer` 枚举与 `ForemanToolSpec.layer` 字段；`system_prompt` 的两组名单从 `available` ∩ 档位谓词派生（`is_env_write_tool ∨ is_service_write_tool` → 「会改动东西的工具」，其余 → 「你能直接用的工具」）
- [x] 冻结测试改写：`foreman.rs` 里 Read / Write 两条逐名断言并成**一条 21 名顺序冻结**（名字与顺序照旧逐条钉，注释搬过去）+ **一条分组派生断言**（两组的并 == 冻结名单、交为空、分组与档位谓词一致）；`env_mode.rs` 的两段清单语义测试不动
- [x] 新增 prompt 真话用例：**值守轮** prompt 不含 `read_conversation` / `run_command`（修 1106 早于 1127 的顺序 bug）；**`deny` 档** prompt 不含任何环境层工具名；既有「按档位说真话」用例（foreman.rs:2139 那条）仍绿
- [x] `power_discipline` 与 `FOREMAN_PERSONA` / `FOREMAN_BASELINE` 文案不动；`gate_decision` 执行时照旧收 `env_mode`（行为零变化——两套分类对 spec 成员本就一致，本票是纯去镜像）
- [x] `make check` 绿
