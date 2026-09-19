# 02: 第 21 个工具 `run_readonly`（先改冻结断言）

**What to build:** 给值班长加**只读取证**的手（决策 237）：白名单 `date` / `ps` / `pgrep` / `lsof` /
`wc` / `tail` / `sample`；**不经 shell**（argv 直出、按命令名判定）；参数两条校验（路径不得越出
文件域、`sample` 的 pid 必须落在「本服务的 pid + 其子进程」集合内）。它属**只读层**，
不受 `env_mode` 档位影响、也不在 `FOREMAN_WATCH_TOOL_DENY` 的拦截面里。

**为什么需要它**：值守轮被 `FOREMAN_WATCH_TOOL_DENY` 收掉了 `run_command`，于是「夜里自己发现
并定死」在工具面上不成立——2026-09-19 实测里 7 次自主唤醒全部止步于「我定不死 / 等你按键」。
决策 232 的那份白名单**没有落点**，直到有一个能执行它的入口。

**Blocked by:** None（与票 01 都在读面，并批省一次构建）

**Status:** pending

- [ ] **先改断言再加能力**（决策 209 的次序）：`foreman.rs` 的冻结工具清单用例与
      `FOREMAN_WATCH_TOOL_DENY` 相关断言先写进 `run_readonly`
- [ ] `FOREMAN_TOOL_SPECS` 追加 `run_readonly`（`Read` 层，第 21 个）
- [ ] `ToolExecutor::run_readonly`：白名单判定 → argv 直出 → 路径参数过文件域 → `sample` 过 pid 集
- [ ] pid 集判定可注入替身（纯函数 + 一条用真子进程的用例），`sample` 的拒绝发生在**启动之前**
- [ ] 命令仍落 `kanban_node_commands`（归属走会话，决策 204④）——审计面看得见每一次取证
- [ ] 用例：白名单外拒（`sh` / `rm`）、分号与管道**没有落点**（不经 shell 的牙齿）、
      `data/` 与域外路径拒、`sample` 对外来 pid 拒而对本进程子进程放行、
      值守轮的工具集里有它、`deny` 档仍广告（它改不了任何东西）

**Notes（实现提示）:**
- 不放 `run_command` 进值守轮再在执行点收：那会造出决策 182 明令不要的形状（模型看得见一个
  调了也没用的东西），也让 `deny` 变成一个要分场景解释的概念。
- `sample` 是这份白名单里**唯一能读走别的进程内存镜像**的一个，故单独一条校验。
