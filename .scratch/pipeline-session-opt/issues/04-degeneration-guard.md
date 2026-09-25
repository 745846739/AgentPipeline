# 04: 模型退化护栏——流式循环检测 + degraded 打标（决策 280）

**What to build:** 模型把同一个片段复读上百遍时（实测：「Playwright 或」×150+、「路由变化路由变化路由变化」），管线不再等它烧完 89 秒才判败——流式输出层检测到循环即取消本轮、打 degraded 标记、立即按决策 278 续接重试。护栏是机制不是配置：阈值常量起步（决策 224 姿态），没有第二种诉求之前不扩契约。

**Blocked by:** 02（degraded 后的续接重试依赖决策 278 的转录保留语义）

**Status:** done（2026-09-25）

- [x] 流式层 n-gram 循环检测：同片段连续重复超阈值 → 取消本轮、判 degraded、立即续接重试（决策 280）
- [x] degraded 打标进 run 行，API 可见（复用 kanban_node_runs 既有列，不新表）
- [x] 阈值与检测窗口在票内设计定稿（常量）并有测试钉住
- [x] 单测（重复片段触发取消）+ integration（触发 → degraded 标记 → 重试已续接）；四门全绿

## Comments

- 来源：同一会话复盘——run41 退化输出 4,322 completion tokens 大半是复读垃圾，现行机制零感知；决策 278 的「转录原样保留」使得 degraded 打标成为必要（错误 turn 需引用「上一轮已判废」的依据）。

- **评审记录（2026-09-25）**：「API 可见」的既有面 = `read_diagnosis`（决策 211③）的 runs 摘要（含 error 字段，值班长读得到）+ 台账查询——run 行 error 列带 `degraded` 标记后即可被消费，无需新端点；SSE `NodeFinished` 不带 error 字段是既有形状，不在本票扩。
