# `run_command` 走 rtk：一个收口、一个开关、一次检测

**Status:** 已裁（**决策 297**，2026-09-26，经两轮拷问 Q1–Q11；落地时按票 06 的判据取当时的空号——294 已被停钮占用，编号在这里一并订正）。实现票见 [`issues/`](issues/)（6 张，
按依赖边 01 → 02 → 03 → 04 → 05 → 06）。

用户诉求原话：「为 run_command 工具增加 rtk 支持（设置页启用，启用前检测本机 rtk 二进制是否可用并提醒）」。

这是决策 185 的自然续篇。185 把「PATH 工具型技能」整体退场，并写明裁决：「二进制怎么用、能不能用，由
`run_command` 与**系统权限**决定，与技能体系无交集」。在此之前，让命令走 rtk 的唯一办法是模型读一份
markdown 技能正文然后**自觉**加前缀——那是把系统级的事实（这台机器上要不要用这个过滤器）寄存在模型的
自觉里。本条把它挪回系统：开关在界面上、改写发生在执行点、可用性在启用前说清。

---

## 1. 收口：所有跑命令的地方合成一个函数

**实测清单（生产代码，不含测试）**：一共 8 处启动外部进程，其中「跑一条命令并收输出」的是 4 处。
`crates/core/src/process.rs` 的两个 helper（`spawn_in_own_process_group` /
`spawn_argv_in_own_process_group`）**已经共用同一条管道** `run_child_to_outcome`
（`crates/core/src/agent/tools.rs:1879`）——`run_command` 与 `run_readonly` 都已经在里面。
**真正绕开这条管道的是两个 `sh -c` 旁路**：

| 旁路 | 命令来源 | 今天的现状 |
|---|---|---|
| `crates/core/src/pipeline/executor.rs:1507` | 项目配置（`project.lint_command` / `test_command_for`） | 有超时，但超时只丢 future、**不杀进程**；从不回填 `process_group_id`，调度器那一刀也够不着 |
| `crates/core/src/pipeline/repair.rs:316` | 同上 | **完全没有超时**；`.output().await` 裸调；台账（`:310`）还漏了 `sanitize_command_line` |

另外 4 处不是「跑一条命令」：`kill`（`process.rs:83`）、`ps`（`tools.rs:3140`）各是一个探测工具，
加内部 `.spawn()`。

**收口到哪一层**：不是「只搬 spawn」，是**整条生命周期收口**。宿主是新模块
`crates/core/src/exec.rs` 里的 `CommandRunner { store, settings, clock, killer, recorder, sse }`——
它不能依赖 `ToolExecutor`，因为闸门那侧只有 `Store` + `Settings` + `Clock`，手里没有 `ToolExecutor`
（今天的 `run_child_to_outcome` 长在 `ToolExecutor` 上，用的是 `self.recorder` / `self.sse` /
`self.killer` / `self.settings`）。`process.rs` 保持「纯启动 + 终止」层，职责不动。

收口兑现的是这四件事，闸门也一并拿到：**进程组 + 心跳 + 超时杀干净 + 脱敏台账**。
两处既有缺陷跟着修（它们是「两个实现」的产物，不是新需求）：

1. repair 闸门**从无上限**变成受 `test_command_timeout_sec` 管；
2. runner 闸门超时后**真的杀进程组**，并回填 `process_group_id` 使调度器那一刀够得着它。

**闸门超时上限：复用 `test_command_timeout_sec`（默认 600s），不新增键。** 理由：同一批命令在
develop 闸门（`executor.rs:1509`）今天已经受这个上限管，所以「测试套件超过 600s」的项目今天就已经
在那个闸门失败了——给 repair 同一个数不是新增风险。新键要动 `Settings` / `Settings::default` /
`PipelineOverrides` / `set!` 宏清单四处，而其中宏清单那份**漏写是静默的**（决策 258 的学费）：
为一个没有证据的需求付那个代价不划算。**这一条是本方案里唯一可由用户推翻的默认。**

**出口检查不进收口。** 它留在调用点（`run_command`，`tools.rs:1837`）——闸门命令来自项目配置、
不是模型写的，而出口策略的对象是「模型想访问网络」（决策 179 的范围）。顺手给闸门加检查是改既有语义，
不是收口。

**不做**：`kill` / `ps` 不收（它们是系统调用式的探测，不是「跑一条命令」；`ps` 今天连超时都没有，
把它收进来属于把范围做大而不是做对）。`run_readonly` 已在管道里，保留。

---

## 2. 改写：挂在收口点上，由调用点声明

形态是一个参数：`Rewrite::Rtk | Rewrite::None`，**函数是同一个**，挂不挂由调用点说清。
这样「hook 只挂一次、挂在那一个函数上」成立，同时不把改写强加给不该改写的路径。

| 调用点 | 改写 | 理由 |
|---|---|---|
| `run_command`（值班长 + 流水线，同一个执行函数） | `Rtk` | 题面本体；命令是模型随手写的探索命令，输出是给模型的，过滤是净收益 |
| 闸门（`executor.rs` / `repair.rs` 的 lint + test） | `None` | 见下 |
| `run_readonly`（argv 直出） | `None` | 决策 232 的安全面是「命令名与参数是两个独立的数组元素」——把 `argv[0]` 换成 `rtk`，`sh -c "date; rm -rf x"` 那种「按命令名判定」的性质就丢了 |

**闸门为什么不挂（实测证据）**：exit code 原样透传（假 `cargo test` 的 101、假 `pytest` 的 1 都保住），
但**输出会被重排成摘要**——`pytest` 的 `assert 1 == 2` **丢了文件与行号**；而当运行器的输出不是 rtk
认得的样子时，输出会被换成**零行**（只打一行的假 cargo）或一句**误导性摘要**（`Pytest: No tests
collected`）。闸门的输出同时是两样东西：模型改代码的**唯一证据**，以及 `<task_dir>/gate-output-<stage>.log`
那份**取证物**（决策 211 一脉）。一条命令只跑一次，拿不到「既过滤又原样」的两份——过滤后落盘等于把
取证物润色了。**给模型省 token 不能拿证据的面做交换。**

---

## 3. 改写怎么实现：复用 rtk 自己的改写器

`rtk` 0.42.4 按 **argv** 过滤，不接受带空格的整串（`rtk "ls -la /tmp"` → exit 127），所以「整串前面
加个 rtk」这条路是死的。它自带改写器 `rtk hook claude`（Claude Code 的 PreToolUse 钩子）：

```
stdin : {"tool_name":"Bash","tool_input":{"command":"<原命令>"}}
stdout: {"hookSpecificOutput":{…,"updatedInput":{"command":"<改写后>"}}}
空输出 = 不改写
```

实测它自带三件我们不想自己维护的东西：**链式逐段加前缀**（`cd x; ls; git status` →
`cd x; rtk ls; rtk git status`）、一份**允许名单**（`git`/`ls`/`grep`/`find`/`wc`/`diff`/`cargo`/…；
对 `cd`/`rm`/`cp`/`mv`/`export`/`source`/`chmod`/`eval` 以及 `test`/`read`/`log`/`run`/`config` 一律不加）、
以及**幂等**（已是 `rtk …` 就回空输出，故模型自己加了前缀也不会变成 `rtk rtk …`）。

**为什么不自写前缀器**：两种失败的形状不同。协议变了 → 解析不出 → 不改写 → 命令照常跑（只丢优化）；
名单写错 → **命令坏掉**——`test -f x` 撞 rtk 自己的 `test` 子命令（实测 `rtk test 1 = 1` 走到 rtk 的
测试运行器上、exit 127）、`env FOO=1 cmd` 撞 `rtk env`。而「哪些命令 rtk 认得」这份清单住在 rtk 里，
抄一份出来就是制造一份会漂的清单（决策 258 的教训）。

**收口形态**：`rtk::rewrite(&str) -> Option<String>`——非零退出 / 空输出 / JSON 解析失败 / 2s 超时
一律 `None`。**失败一律放行**：原样执行，不阻断命令。

**它不只是加前缀**（这是台账要记两份的原因）：`cat X` → `rtk read X`；
`python3 -m pytest -q` → `rtk pytest -q`。执行的是**另一条命令**，不是同一条命令的包装。

---

## 4. 可用性：怎么问、问什么、钉成什么

**判据三条**（第三条是关键）：

1. 解析到一个绝对路径；
2. 它 `--version` 能跑；
3. `rtk hook claude` 喂一条 `ls`，能回一段**可解析的**改写。

第 3 条把本设计里唯一一处「依赖没文档的协议」变成**开箱可见的事实**：rtk 版本太老、没有 `hook`
子命令、改写被关掉——都在设置页就说「这台机器上的 rtk 不能改写」，而不是等第一条命令悄悄没被优化。
静默失效是这类耦合最坏的形状。

**怎么找（按顺序）**：服务进程的 `PATH` → 已知目录（`/usr/local/bin`、`/opt/homebrew/bin`、
`/opt/local/bin`、`~/.local/bin`、`~/.cargo/bin`）→ **设置页手填路径**（兜底）。

> **落地订正（票 04）**：手填路径落地为**优先**而不是兜底——填了就用它，没填才走上面那条 PATH →
> 已知目录。票 04 的判据原话是「手填路径**能覆盖**自动解析」，而兜底语义下装有新旧两个 rtk 的人
> 无法指定用哪个（自动解析总是先赢），这条判据就成了空话。填错时**如实报错、不静默回落**。
> 决议正文（决策 297）与 `docs/operations.md` 记的是订正后的语义。
**不问用户的登录 shell**：`$SHELL -lc 'command -v rtk'` 会在服务进程里**静默执行用户的 rc 文件**
（副作用 + 数百毫秒），而决策 185 之后本仓对「偷偷扫 PATH」一贯的处置是删掉。失败可见、可修就够了。

**钉成什么**：启用时把解析到的绝对路径写成一个**私有 shim 目录** `{home}/rtk-shim/`，里面
**只有一个** `rtk` 符号链接；运行期由收口函数把这个目录前置进**子进程**的 `PATH`
（`cmd.env("PATH", …)`）。

- 为什么必须钉：桌面壳由 Finder 直接 exec（`crates/desktop/src/main.rs:44`），继承 launchd 的最小
  PATH（本机 `launchctl getenv PATH` 为空 → `/usr/bin:/bin:/usr/sbin:/sbin`），**`/usr/local/bin/rtk`
  不在里面**；命令行起 `agent-pipeline serve` 则继承 shell 的 PATH。于是「本机装了 rtk」与「这个服务
  能用 rtk」是两个答案，而后者才是唯一有意义的那个。钉住它，**「启用时说可用」和「真跑起来可用」才是
  同一件事**——否则最坏的形状是设置页说可用、命令全 127。
- 为什么不是把 `/usr/local/bin` 前置：那个目录里还有一堆别的二进制，前置它会顺手改掉别的命令的解析
  （`python3` 之类）。shim 只影响 `rtk` 一个名字。
- 为什么必须靠 PATH 前置而不是把路径插进命令串：改写器吐出来的是裸 `rtk`，靠 PATH 找；对每段做字符串
  手术（把 `rtk ` 换成绝对路径）既脆又要在引号里做手术。

**运行期不在场**（被卸载 / shim 失效）：**原样执行 + 留痕**。每个班次最多一条 `tracing::warn`，
不阻断命令——优化器不可用不该升级成整条命令失败。

---

## 5. 设置

DB 单行表（`CHECK (id = 1)`），照 `kanban_foreman_watch` / `kanban_notify_channel` 的三动作形状
（读 / 写 / 清）。**行缺席 = 默认，默认关。**

**粒度是全局**：rtk 是「这台机器上要不要用这个二进制」的**机器事实**，不是「这个阶段在干什么」的
**阶段语义**——阶段级正是为后者设的（决策 206 的原话）。阶段级要多付一列迁移 + 三层解析 + 前端控件 +
跨语言 spec 表，收益是今天说不出谁需要。

**端点**：

| 端点 | 行为 |
|---|---|
| `GET /rtk` | 存的状态 + **活体探测**（路径 / 版本 / 可用 / 原因） |
| `PUT /rtk` `{enabled}` | 启用时先探测；**探测失败不拦**，两个结果都 200，响应体带 `{ok, message, path, version}` |

带活体探测的理由是 `lanToggle` 那条纪律：**重读目标态才算数**，不缓存上一次的探测结果（决策 257 是
「漏读」的学费）。探测失败**不拒绝保存**：决策 185 已裁「二进制能不能用由系统权限决定，本系统不另设
一层」，一个输出优化器不该有权限拦人；而且开发机上「先开开关、后装二进制」是常见顺序。界面把失败原样
摆出来（原因 + 找到的路径 + 手填框），保存的是「已启用，但本服务当前找不到 rtk」。

**界面**：新页 `settings-tools`（「命令执行」），落落地页的「怎么跑」类目——照决策 287 给值守轮开关
单开一页的先例。rtk 不属于任何一个现有页面的主题，塞进哪一页都会让那页主题变糊；而且「机器级的命令
执行设置」这一类后面还会长（出口策略、超时都在这一带）。

**一句话带上**：启用后 rtk 技能就成了多余的说明书（模型自己加前缀时改写器幂等，不会双加），可自行
卸载——但**不自动改用户的技能配置**。

---

## 6. 台账与出口

**加一列** `original_command TEXT`（迁移 `0034_run_command_original_command.sql`，可空）。语义：
**只有真的发生过改写才写**；NULL = 按原样跑（未启用 / 调用点传 `None` / rtk 不在场，这三件事对台账
是同一件事）。

界面默认显示原串，能展开看实际执行的串。三种记法里只有这个不丢信息：只记实际执行的串 → 原串丢了
（而原串是模型想要的东西）；只记原串 → `cat` 与 `rtk read` 的输出不一样，排障会看错；记两行 →
混淆「跑了几条命令」这个计数。

**出口检查的顺序是不变量**，要有测试钉住：

```
check(原命令)  →  改写  →  落台账  →  spawn
```

被拒的命令根本不改写、也不启动。顺带补一条与本次功能无关的既有缺口：`egress::WRAPPERS`
（`crates/core/src/agent/egress.rs:130`，8 项）要加上 `"rtk"`——今天 `rtk curl http://x` 会被判成
**本地命令**放行，也就是「加前缀」本身就是一条出口缺口，而模型自己写 `rtk curl …` 时同样成立。

---

## 7. 明确不做

- 闸门 / `run_readonly` 不做改写；不收 `kill` / `ps`；不把出口检查塞进收口。
- 不新增超时键（闸门复用 `test_command_timeout_sec`）。
- 不问用户的登录 shell、不扫 PATH 找二进制（决策 185 的延续）。
- **不新增可测试性接缝**（`docs/testing.md` 仍是五条）：改写是「命令串 → 命令串」的纯映射，做成收口
  函数 + 直接喂串的单测，比再加一条 trait 更便宜也更准。
- 不动 rtk 自己的配置（tracking / tee / telemetry 是 rtk 的域）。实情记一笔：启用后流水线的每条命令
  都会进 rtk 的本地历史（默认 90 天）与 tee 目录（失败输出，最多 20 个文件）；设置页不替 rtk 承诺
  任何事。

---

## 8. 验证

| 判据 | 层 | 落在哪 |
|---|---|---|
| 改写是纯映射：链式逐段 / 已带前缀回空 / 空输出与解析失败回 `None` | L1 | `crates/core/src/rtk.rs` 内联单测 |
| 可用性三条判据各自的失败原因可归因（找不到 / 版本跑不起来 / 改写不可解析） | L1 + L2 | `rtk.rs` 单测 + `crates/app/tests/integration/api_contract.rs` |
| `rtk curl http://x` 被判为出口（WRAPPERS 补齐） | L1 | `crates/core/src/agent/egress.rs` 既有 wrapper 用例组 |
| 出口判定在改写**之前**、被拒命令不启动也不改写 | L2 | `crates/core/tests/integration/egress.rs` |
| 台账两份：改写发生过 → `original_command` 非空；未启用 / 闸门 → NULL | L2 | `crates/core/tests/integration/env_mode.rs` 同族 |
| 闸门现在有进程组、超时**杀干净**、心跳在跑（两处既有缺陷的回归） | L2 | 替换/加强 `.scratch/timeout-actually-stops/` 那批用例 |
| repair 闸门有超时上限 | L2 | `crates/core/tests/integration/` 闸门用例 |
| rtk 不在场 → 命令原样执行且 exit code 正常 | L2 | core 集成（把 rtk 从 PATH 摘掉） |
| 开关三态（未启用 / 启用可用 / 启用不可用）：`GET /rtk` 活体探测、`PUT /rtk` 失败也 200 | L2 | `crates/app/tests/integration/api_contract.rs` |
| 页面：开关、探测结果三态、手填路径兜底、落地页目录项 | L6 | `frontend/src/routes/SettingsTools.test.ts` + `lib/rtkToggle.test.ts` + `SettingsLanding.test.ts` |
| 端到端：启用 → 跑一条命令 → 台账出现 `original_command` | L7 | `frontend/e2e/` |

另：`process.rs` 的 `set_process_group` 写库失败时 `?` 会在 collector 建起来之前返回、已 spawn 的
`Child` 被丢掉（没有 `kill_on_drop`）——收口时顺手用 `kill_on_drop(true)` 或先建 collector 兜住它。

## 9. 文档面

- `docs/decisions.md` 追加 **297**（引用 185 / 206 / 211 / 232 / 258 / 287）——**294 是裁时的占位号**，落地时已被停钮（决策 294）占用，按票 06 的判据改成当时的空号 297。
- `docs/agents.md`：`run_command` 一节 + 决策 185 那段（「二进制怎么用由 run_command 与系统权限决定」）
  现在有落点了。
- `docs/operations.md`：命令执行的台账语义（原命令 vs 实际执行、rtk 不可用时的降级）。
- `docs/implementation.md` §11.5（新列）+ §11.7（新端点）。
- `docs/testing.md`：用例目录 + 决策↔测试映射（收口那两条回归是重点）。
- `docs/glossary.md` 两个新词：**改写**（命令在落台账与启动之前被换成另一条经过滤器的命令）、
  **shim 目录**（只放一个名字的私有目录，用来把二进制钉进子进程的 PATH）。
- `design/frontend-design.md` §4.3（落地页目录）+ §12.3（行为 → 实现位置；`behavior-map.test.ts`
  会拦悬空引用）。顺手补上 `SettingsLanding.test.ts` 漏掉的「值守轮」条目（它今天只列六项）。

---

## 附：本轮拷问的实测记录（实现时对照）

```
rtk 0.42.4 @ /usr/local/bin/rtk      桌面壳从 Finder 启动：launchctl getenv PATH 为空 → /usr/bin:/bin:/usr/sbin:/sbin

整串前缀不可行      rtk "ls -la /tmp"                    → exit 127
未知二进制透传      rtk echo hi                          → exit 0
rtk 自己的子命令名撞 shell 命令：
  rtk test 1 = 1                                        → exit 127（走到 rtk 的测试运行器）
  rtk env / log / run / config / read / json / init     → 同名子命令
  rtk cd /tmp                                           → exit 0（无输出）
改写器覆盖          简单命令与 `;` / `&&` 链：改写
                    重定向 / 子壳 / 命令替换 / for 循环：整串放弃
                    `git log | grep fix` → 只改后半段；`git log | head -5` → 全不改
单次开销            rtk hook claude ×100 = 4.49s（约 45ms）
不只是加前缀        cat X → rtk read X;  python3 -m pytest -q → rtk pytest -q;  npx eslint . → rtk lint .
闸门改写后的输出    假 cargo：exit 101 保住，错误块保住，丢 Compiling 与结尾句
                    假 pytest：exit 1 保住，assert 丢文件行号
                    形状不认识时：输出变零行，或一句 Pytest: No tests collected
```
