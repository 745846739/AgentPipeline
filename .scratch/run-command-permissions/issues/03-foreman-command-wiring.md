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

**Status:** done

- [x] 值班长能跑命令且**命令真的落库**（`task_id` NULL + `session_id`），并能从会话维度读出来
- [x] 域 = `home.root()`；`data/` 与 `logs/` 在文件工具下不可读不可写
      （测试：`read_file` 打 `data/agentpipeline.db` 被拒）
- [x] L2 卸载不写 `{root}/tasks/.context`（有测试）；大输出命令的卸载落在会话维度
- [x] `FOREMAN_ENV_TOOLS` 与 `FOREMAN_SERVICE_WRITE_TOOLS` 两段清单落常量；
      `FOREMAN_PERSONA` / `FOREMAN_BASELINE` / 工具纪律段（`foreman.rs:624-631`）按档位改写——
      不再说「读不到文件系统，也不能执行命令」
- [x] `tests/foreman.rs` 那条「工具集恰为约定的两个」已由 `foreman-capabilities/01` 改成清单驱动，
      本票补的是「按档位广告」的断言与「`deny` 档下命令跑不起来」的取证
- [x] `docs/operations.md` 的残余风险改写由票 04 做，本票只提供事实（哪些路径可达、哪些不可达）

## 交付

**① 域与补偿**。`ToolCallContext` 的 `worktree_path` / `task_dir` 都是 `home.root()`，
`default_cwd` 为 `None`（行为上回落到同一个值）。`FileToolPolicy::foreman_file_policy(home_root)`
按**路径前缀**把 `{root}/data` 与 `{root}/logs` 加进 `deny_paths`（原来的名单是**模式**：
`.env*` / `*.pem` / `id_rsa*` / `~/.ssh`，盖不住一个 `.db` 文件）。匹配语义确认过：`denied_pattern`
对**原始与 realpath 两个形态**各查一次，且是「路径以该前缀开头」的判定。
取证：`tests/env_mode.rs::the_foreman_domain_covers_the_home_but_not_data_or_logs`
（家目录里的普通文件读得到写得进；`data/` 与 `logs/` 下**读与写都拒**，且拒绝报文点名是哪一条命中的）。

**② 命令日志**。`foreman_tooling` 给执行器接上 `with_recorder(store)`——命令落
`kanban_node_commands`，`task_id` 为 NULL、`session_id` 是当前班次（迁移 0012 的两列）。
`CommandSource` 复用 `Agent`，没有第三个变体。取证：`tests/env_mode.rs` 的
`an_auto_command_lands_under_the_session_and_nowhere_else`（同时断言**不**出现在任何任务的
命令列表里、另一个班次读不到）。

**③ L2 卸载的空 task_id 守卫**。`ToolExecutor::offload_dir` 按归属选维度：有 task_id 走
`home.context_dir(task_id)`，否则必须**有会话**才落 `{root}/foreman/context/{session}/`——
两者都没有时**报错**，不退化成 `{root}/tasks/.context`（那一层是所有任务共用的，写进去会污染
下一个真实任务的工作区）。`prepare_output`（命令输出）与 `apply_l2_offload`（通用）共用它。

**④ 读取面**。`GET /foreman/commands?session=`（会话维度的只读面，含被出口策略拒掉的命令）。
契约在 `crates/app/tests/api_contract.rs::foreman_commands_are_readable_from_the_session_dimension_only`：
归属列 `task_id` 是 `null`，另一个班次读不到，不存在的班次给空列表而不是 404。

**两段清单落常量**：`agent::tools::ENV_TOOLS` / `ENV_WRITE_TOOLS` / `SERVICE_WRITE_TOOLS`
（名字去掉 `FOREMAN_` 前缀的理由见决策 206 的落地标注），人格与工具纪律段按档位改写
（`the_system_prompt_tells_the_truth_about_what_it_can_do_in_each_tier`）。
