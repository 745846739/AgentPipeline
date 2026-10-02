# 01: 工具级执行日志(B3 切口)——工具调用进出日志流,非命令工具补时长

**What to build:** agent 执行任务时,每次工具调用在**日志流**里留下开始、结束、耗时,
与既有「模型请求派发/收场」日志同构。

> **范围修订(2026-10-02 归因发现)**:命令类工具其实已有 `kanban_node_commands` 台账
> (started_at / finished_at / duration_ms 齐全)——当天 8 段 ~7.5 分钟静默间隔就是靠它
> 破案的(全是 playwright e2e 跑)。真正的缺口是两条:
> ① **日志流里没有工具调用记录**:巡检习惯翻文件日志/journal,不知道(也不该必须知道)
> 要另查 DB 台账——模型请求有 recording 日志行,工具调用没有,这是「同样的事实在两个
> 介质里只活一处」的观测断层;
> ② **非命令类工具**(write_file / read_task / 读台账类)没有任何时长记录。
> 故本票收窄为:补日志行 + 补非命令工具的时长;DB 台账不动。

**Blocked by:** None(可立即开始)

**Status:** ready-for-agent

- [ ] 每次工具调用(含命令类与非命令类)有 start 行与 end 行,含工具名、run、attempt、
      耗时;命令类的 end 行与 kanban_node_commands 台账可对上(同一次执行、同一时长)
- [ ] 日志可离线聚合:给定一个 task,能从日志流算出各阶段「模型时间 / 工具时间 /
      无日志间隔」三段分布(脚本或临时查询即可,不要求界面)
- [ ] 部署到 106 后,新任务不因新增日志产生明显 IO 压力
      (storage-io-budget 的约束仍成立)
- [x] 不改 attempt 语义(B8 另案),不动 recording 落库口径
      —— **行上无 attempt 字段**(code review 指出):ToolCallContext 没有它,补齐要翻
      调度层;B8 已把 attempt 计数器本身定为混义,等 B8 釐清了语义再补字段,不抢跑。
- [x] **时长口径已核**(code review):日志收场行的 duration_ms 覆盖「分发→L2 卸载」
      全程,kanban_node_commands.duration_ms 只量 runner.run 一段——**近似相等、
      不严格相等**,对账以「同一次执行、同一量级」判,不逐毫秒比。
