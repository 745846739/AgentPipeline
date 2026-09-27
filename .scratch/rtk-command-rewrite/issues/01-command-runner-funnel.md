# 01: 收口——`CommandRunner` 接管两个工具路径（行为逐字不变）

**What to build:** 把「启动一条命令 → 流式采集 → 超时收口 → 台账回填」这条管道从 `ToolExecutor` 里
搬出来，成为**不依赖 `ToolExecutor`** 的 `CommandRunner`（新模块 `crates/core/src/exec.rs`）。宿主
必须是独立结构而不是 `ToolExecutor` 的方法：闸门那侧手里只有 `Store` + `Settings` + `Clock`，没有
`ToolExecutor`，而本票之后的票要让闸门也走这条管道。`run_command`（`sh -c`）与 `run_readonly`
（argv 直出）迁入，两者仍只在「怎么启动」上不同；管道上留一个 `Rewrite::Rtk | Rewrite::None` 参数，
**本票一律传 `None`**。两处闸门旁路（`executor.rs` / `repair.rs` 的 `sh -c`）**仍在原地**，不在本票范围。

**Blocked by:** None（可立即开始）

**Status:** done（已实现；两条判据按落地实情改写，见下）

- [ ] `CommandRunner` 落地，依赖是 `Store` / `Settings` / `Clock` / `ProcessKiller` / `CommandRecorder`
      （可选 SSE），**不依赖 `ToolExecutor`**
      → **不勾**（依赖清单变了，不是漏做）：落地的必填依赖只有 `ProcessKiller`，其余全走可选接线
      （`with_recorder` / `with_sse` / `with_heartbeat_interval` / `with_rtk_store`）。**没有 `Settings`**——
      收口只该知道「这条命令怎么跑」，超时上限由调用点按自己的语义算好传进来（agent 侧
      `effective_run_command_timeout`、闸门侧 `test_command_timeout_sec`），收一份 `Settings` 只会让
      收口有机会**自己**重新解释一遍策略；**也没有 `Clock`**——时长用 `Instant` 度量，因为修复闸门
      那一侧手里根本没有 `Clock` 可传（显式偏离 spec §1 的草图）。`Store` 只在「读 rtk 开关」一处
      需要。**「不依赖 `ToolExecutor`」这一半成立**：`exec.rs` 里 5 处 `ToolExecutor` 全是注释
      （说明为什么不依赖），类型层引的是台账那几个共享类型（`CommandFinish` / `CommandRecorder` /
      `CommandSse`），不是执行器。见决策 297「**它也不收 `Settings`**」那段。
- [x] `run_command` 与 `run_readonly` 都经它执行；两者唯一的差别仍是启动形态（`sh -c` vs argv 直出，
      决策 232 的安全面不变）
- [ ] `Rewrite` 参数存在且本票一律 `None`——本票**不产生任何改写**
      → **不勾**：参数在，但**终态里 `run_command` 传的是 `Rewrite::Rtk`**——票 01–06 同批落地，
      这一条只在票 01 自己的时间边界内成立。终态里成立的那一半有测试各钉一处：
      `an_argv_command_is_never_rewritten`（`run_readonly`）、`the_repair_gate_never_rewrites_its_command`
      （闸门）。
- [x] 既有测试**全绿且不改断言**。这是「行为逐字不变」唯一诚实的判据：任何需要改断言的差异都说明
      搬错了地方
      → 机器核对：`git diff` 在全部测试文件里**删掉的断言行是 0**（改动只有构造点/签名的管线，
      如 `ToolExecutor::new(...)` 收进 `build_executor`、`run_repair_gate` / `finish_repair_round`
      多收 `settings` + `killer`、`CommandRecord` 多一格 `original_command: None`）。前端有**两处**
      断言值变化，都是票 05 明写的要求（落地页目录项、顶栏高亮集合），不是搬错了地方。
- [x] `process.rs` 保持「纯启动 + 终止」层，职责不动（它是决策 143 的可测试性接缝③）
      → 只多了一个纯函数 `child_path(prefix, base)`（PATH 前置的合成规则）与 `ChildEnv` 一格，仍是
      纯 spawn/terminate，没有任何台账/超时/改写职责。
- [x] 顺手兜住一条既有的漏杀路：`set_process_group` 写库失败时 `?` 会在流式采集器建起来之前返回，
      已 spawn 的 `Child` 被丢掉（全仓无 `kill_on_drop`）——用 `kill_on_drop(true)` 或先建采集器兜住
      → 走了第三条：`exec.rs::ChildGuard`（析构里 `start_kill`），没被 `take()` 走的那一份在析构时
      收拾。不在 `Child` 上开 `kill_on_drop(true)` 是因为收口正常路径下要把 `Child` 交出去给流式
      采集器，而 `kill_on_drop` 是 `Child` 自身的属性、交出去以后也一直挂着——它会改变正常路径的
      语义，不只是兜底那一格。
- [x] 四门（`make check`）+ 交付说明

## Comments

- 本票是 prefactor（「先让改动变容易，再做容易的改动」）：不做它，改写就得在 `run_command` 里长一遍、
  闸门里再长一遍——那就是给下一次漂移留位置。
- 这条管道的抽取理由在 `tools.rs` 里已经记过一次：`run_command` 与 `run_readonly` 的副本曾漂移，
  只有一支在超时后补了杀进程组。本票把同一件事推到闸门。
- 生产代码一共 8 处启动外部进程，其中「跑一条命令并收输出」的是 4 处。`process.rs` 两个 helper 早已
  共用这条管道，**真正绕开的是两个 `sh -c` 旁路**。另外 4 处（`kill` / `ps` / 内部 `.spawn()`）不是
  「跑一条命令」，不收（见 spec §1）。
