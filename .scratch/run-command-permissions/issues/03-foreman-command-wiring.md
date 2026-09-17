# 03: 值班长的命令与文件面接入——域、补偿、日志

**What to build:** 按决策 206 把值班长的**环境层**真正接通。四件事：

**① 域与补偿**。`ToolCallContext` 的 `worktree_path` / `task_dir` 已经是 `home.root()`
（`foreman.rs:416-427`），`default_cwd` 是 `None`（行为上回落到 `worktree_path`，即同一个值）——
写明即可。`FileToolPolicy` 的 `deny_paths` **按路径前缀**加 `{root}/data` 与 `{root}/logs`：
`data/agentpipeline.db` 明文存 provider 密钥（决策 112），而默认名单是模式
（`.env*` / `*.pem` / `id_rsa*` / `~/.ssh`，`file_policy.rs:46-55`），盖不住它。落地时确认
`deny_paths` 的匹配语义（前缀还是精确）并按需补齐，写进交付说明。

**② 命令日志**。值班长的命令要落 `kanban_node_commands`：`task_id` 为 NULL、`session_id` 为当前会话
（两列由 `foreman-sessions` 01 的迁移 0012 给）。今天它**插不进去**——`record_command_start`
（`tools.rs:861-882`）把 `ctx.task_id`（空串）直接绑进 `task_id`，而那一列 `NOT NULL` 且外键指向
`kanban_tasks`，空串会被 SQLite 拒（`0004_project_level_runs.sql:11` 的注释写明理由）。
`CommandSource` **复用 `Agent`**，不加第三个变体（约束在 `task_id` 上，加变体救不了）。

**③ L2 卸载的空 task_id 守卫**。`run_command` 自己的卸载路径（`tools.rs:826` → `prepare_output` →
`home.context_dir(task_id)`）**没有守卫**，空串会落到 `{root}/tasks/.context` 污染下一个真实任务——
`tools.rs:372-374` 的注释已经点名这个后果（守卫今天只加在通用 `apply_l2_offload` 上）。让值班长的
卸载落**会话维度**（`{root}/foreman/.context/{session}/` 这类，落在 `tasks/` 之外），并保留
「空 task_id 不写 `tasks/.context`」这条硬规矩。

**④ 读取面**。`GET /tasks/{id}/commands` 是任务挂的（`routes/tasks.rs:834-869`，含
`command.task_id != id` 的归属校验），值班长的命令在那条路上不可达。给对讲台开一个**会话维度的只读面**
（`GET /foreman/commands?session=`），并在对讲台上能看见（时间线里的工具回执已经有一轮的位置）。

**Blocked by:** `foreman-sessions` 01（命令日志的会话列）、`run-command-permissions` 01、02

**Status:** ready-for-agent

- [ ] 值班长能跑命令且**命令真的落库**（`task_id` NULL + `session_id`），并能从会话维度读出来
- [ ] 域 = `home.root()`；`data/` 与 `logs/` 在文件工具下不可读不可写
      （测试：`read_file` 打 `data/agentpipeline.db` 被拒）
- [ ] L2 卸载不写 `{root}/tasks/.context`（有测试）；大输出命令的卸载落在会话维度
- [ ] `FOREMAN_ENV_TOOLS` 与 `FOREMAN_SERVICE_WRITE_TOOLS` 两段清单落常量；
      `FOREMAN_PERSONA` / `FOREMAN_BASELINE` / 工具纪律段（`foreman.rs:624-631`）按档位改写——
      不再说「读不到文件系统，也不能执行命令」
- [ ] `tests/foreman.rs` 那条「工具集恰为约定的两个」已由 `foreman-capabilities/01` 改成清单驱动，
      本票补的是「按档位广告」的断言与「`deny` 档下命令跑不起来」的取证
- [ ] `docs/operations.md` 的残余风险改写由票 04 做，本票只提供事实（哪些路径可达、哪些不可达）
