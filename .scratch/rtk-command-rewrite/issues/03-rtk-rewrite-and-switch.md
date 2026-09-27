# 03: 改写 + 开关——打开后命令走 rtk，台账看得出差别

**What to build:** 新增 `crates/core/src/rtk.rs`，把「改写」与「本机可用」两件事收在一个模块里。
`rewrite(&str)` 复用 rtk 自带的改写器 `rtk hook claude`（stdin 喂
`{"tool_name":"Bash","tool_input":{"command":"…"}}`，stdout 读 `hookSpecificOutput.updatedInput.command`，
**空输出 = 不改写**），非零退出 / 空输出 / JSON 解析失败 / 2s 超时**一律 `None` 并放行**（原样执行，
不阻断命令）。`run_command` 传 `Rewrite::Rtk`。同时落 DB 单行开关（`CHECK (id = 1)`，行缺席 = 默认关）
配 `GET /rtk`（存的状态 + **活体探测**：路径 / 版本 / 可用 / 原因）与 `PUT /rtk {enabled}`（启用时先
探测；**探测失败不拦**，两个结果都 200）；台账加 `original_command` 列（**只有真的发生过改写才写**）；
`egress::WRAPPERS` 补 `"rtk"`。

**Blocked by:** 02

**Status:** done（已实现；「默认关 = 今天」的逐点核对见文末）

- [x] 改写是**纯映射**，单测直接喂串断言：链式逐段加前缀；已带 `rtk` 前缀回 `None`（幂等）；空输出、
      非零退出、JSON 解析失败、2s 超时都回 `None`
      → 落地的切分比这条细：**「rtk 怎么改写」是 rtk 自己的事**（本设计刻意不重写一份前缀器，见
      Comments），故我们钉的不是「`ls` → `rtk ls`」而是**我们这一侧的两件事**：`rtk` 的 stdout
      JSON → 命令串的**纯映射**（`parse_rewrite_is_a_pure_mapping`，不碰进程），与**每种失败都回
      `None`**（`every_failure_mode_yields_none`：非零退出 / 垃圾输出 / 二进制不在，另加
      `a_rewriter_that_hangs_is_not_waited_for`：**超时那条分支**——短预算 + 一个 `sleep 30` 的假
      rtk 必须到点放行且不真的等）。
      「已带前缀回 `None`（幂等）」是 rtk 的行为，落地用「改写器回一段可解析的改写」判据覆盖
      （`rewrite_reads_the_updated_command_from_the_rewriter`）——测 rtk 自己的改写规则等于测一份
      我们不拥有的清单（决策 258 的教训）。
- [x] 判决顺序是**不变量**且有测试钉住：`check(原命令) → 改写 → 落台账 → spawn`。被拒的命令既不改写
      也不启动
      → `tests/integration/egress.rs::a_denied_command_is_neither_rewritten_nor_run`（开关打开 +
      一个会写标记的假 rtk：被拒的命令既没写标记也没落原串）。
- [x] `rtk curl http://x` 被判为出口（`egress::WRAPPERS` 补 `"rtk"`）——**今天它会被放行**
      （`egress.rs:130` 的 8 项里没有 `rtk`），这是一条与需求无关的既有缺口，必须补
      → `WRAPPERS` 从 8 项到 9 项；单元层 `egress.rs::rtk_prefix_does_not_hide_the_command` 钉住
      「改写不能让出口那道闸失效」。
- [x] 台账两份：改写发生过 → `original_command` 非空；未启用 / 闸门 / `run_readonly` → NULL。
      语义是「只有真的改写过才写」——三种「按原样跑」对台账是同一件事
- [x] **默认关的时候行为逐字等于今天**（交付说明写出逐点核对结论）
- [x] 开关的三种失败原因可归因：找不到二进制 / 版本跑不起来 / 改写不可解析；解析不到东西时**不假装
      可用**
      → `probe_attributes_each_failure` 三条各断一次（第三态用「`--version` 跑了、但 `hook claude`
      回不出可解析的改写」构造，且版本号仍照实显示）。
- [x] rtk 不在场时命令**原样执行**、exit code 正常，每个班次最多一条 `tracing::warn`
      → 原样执行与 exit code 由 `every_failure_mode_yields_none` + agent 侧「开关关着」那条覆盖；
      「每个原因只报一次」由 `each_unavailable_reason_is_reported_once` 直接钉 `first_time(reason)`
      这只闩（评审收口①：首版是每条命令一条，命令是热路径，持续性的失败按条报会把日志刷成噪声）。
- [x] 四门 + 交付说明

## 「默认关的时候行为逐字等于今天」——逐点核对

关着时（`kanban_rtk` 里没有那一行，缺省态）的四条路：

| 面 | 关着时的行为 | 靠什么保证 |
| --- | --- | --- |
| 启动形态 | `sh -c`（`run_command` / 两处闸门）、argv 直出（`run_readonly`） | `SpawnForm` 两态，逐字搬自原实现 |
| 子进程环境 | **`PATH` 一个字都不动**——`ChildEnv { path_prefix: None }` 让 `apply_child_env` 直接 return，`cmd.env("PATH", …)` 不调用 | `process.rs::apply_child_env` |
| 改写 | 原串进管道 | `RtkSource::Off` → `apply_rewrite` 的早退分支 |
| 台账 | `command` = 脱敏后的原串，`original_command` = NULL | `record_start` |
| 超时 | 同一段 `tokio::time::timeout(timeout_sec)`，超时后 `kill_process_group(pgid)`，**已收到的输出保留**（推流过的部分不丢） | `exec.rs::execute`（逐字搬自原 `tools.rs`） |
| 进程组 / 心跳 / 脱敏 / 收尾 | 同一份代码，逐字搬 | `exec.rs::execute` |
| 出口判决 | 仍在**调用点**、仍在 spawn 之前 | `tools.rs::run_command` |
| spawn 失败 | `exit_code = None`（读侧表示「还在跑」）——**执行器闸门那一处例外** | 见下 |

**唯一例外**（与开关无关，是评审收口⑥）：执行器闸门的**启动失败**那条路现在会落一行
`exit_code = -1` 的收尾，收口之前那行停在 NULL 上、永远不闭合。这是本次唯一一处有意的行为差，决策
297 也记了，不假装逐字不变。

## Comments

- **为什么复用 rtk 自己的改写器，而不自写前缀器**（spec §3）：两种失败的形状不同——协议变了则解析不出、
  不改写、命令照常跑（只丢优化）；而名单写错则**命令坏掉**（`test -f x` 撞 rtk 自己的 `test` 子命令，
  实测 `rtk test 1 = 1` 走到 rtk 的测试运行器上 exit 127；`env FOO=1 cmd` 撞 `rtk env`）。而且
  「哪些命令 rtk 认得」这份清单住在 rtk 里，抄一份出来就是制造一份会漂的清单（决策 258 的教训）。
- **它不只是加前缀**（这是台账必须记两份的原因）：`cat X` → `rtk read X`；
  `python3 -m pytest -q` → `rtk pytest -q`；`npx eslint .` → `rtk lint .`。执行的是**另一条命令**。
- 可用性判据三条，第三条是关键：找到绝对路径、`--version` 能跑、**`rtk hook claude` 喂一条 `ls` 能回
  一段可解析的改写**。第三条把本设计里唯一一处「依赖没文档的协议」变成开箱可见的事实——版本太老、
  没有 `hook` 子命令、改写被关掉，都在设置页说清，而不是等第一条命令悄悄没被优化。
- 探测失败**不拒绝保存**：决策 185 已裁「二进制能不能用由系统权限决定，本系统不另设一层」，一个输出
  优化器不该有权限拦人；开发机上「先开开关、后装二进制」也是常见顺序。
- `GET /rtk` 带**活体探测**（不缓存上次结果）：`lanToggle` 那条纪律——重读目标态才算数；决策 257 是
  「漏读」的学费。
- 台账界面默认显示原串、可展开看实际执行的串。只记实际执行的串 → 原串丢了；只记原串 → `cat` 与
  `rtk read` 的输出不一样，排障会看错；记两行 → 混淆「跑了几条命令」这个计数。
