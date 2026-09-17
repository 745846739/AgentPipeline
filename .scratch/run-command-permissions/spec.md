# `run_command` 的权限模式：三档、两层、环境层按档

**Status:** 已裁（**决策 206**，2026-09-17，经一轮拷问 Q3 / Q11 / Q12 / Q16 / Q22 / Q24 / Q28）。
实现票见 [`issues/`](issues/)。

## 1. 一条规则与两张清单

| 面 | 谁说了算 | 三档下的行为 |
|---|---|---|
| **环境层**：`run_command`、`write_file` / `edit_file`、`read_file` / `list_dir` | **档位** | `auto` 直接执行；`ask` 拦截生成提议；`deny` 不广告且拒执 |
| **本服务写接口**（D 层：建任务 / resume / 拍板 / 合入 / 改配置 / 装技能……） | **决策 188 的裁决**，与档位无关 | 恒为提议 + 确认钮，**不可配、不可放** |
| **只读台账工具**（`read_task` / `read_conversation` 及 A 层新增） | 恒直通 | 不受档位影响 |

**为什么这么切**：环境层的失败落在文件与机器里（可回滚、有 `kanban_node_commands` 的全量日志），
状态机写动作的失败会改变流水线的事实（依赖边、worktree 准入、合入门）。前者可以配，后者不可以。
换句话说：`auto` 不属于「权限模式」，它是「取消模式」的一半，而另一半（本服务写接口）不可取消。

白名单因此在执行点分两段（`ToolExecutor::with_allowed_tools`，`agent/tools.rs:337-346` 是**唯一**的
执行点）：

- `FOREMAN_ENV_TOOLS`——档位可放开；
- `FOREMAN_SERVICE_WRITE_TOOLS`——永不放开，只读档位之外的那条规则。

**`deny` 的实现是「不广告」**（从 `tool_defs` 里摘掉），执行点再拒一次兜底：先拒绝再让模型换路只浪费
轮次，而且它的人格会因此在描述一个不存在的能力。

## 2. 档位与分层

三档：`auto` / `ask` / `deny`。**不在 `ask` 与 `deny` 之间插第四档「白名单内自动」**——那会把
`egress_allow_hosts`（约束网络出口，决策 179）与命令权限混成一层，两件事各有各的边界。

分层**两层**：全局默认（`[pipeline]`，不进 UI，照 `allow_dirty_worktree_merge` 那一批今天的做法）
+ 阶段级（`stage_configs` 新列，**含 `foreman` 行**——`PSEUDO_STAGE_KEYS` 今天已含它，
阶段配置页今天已能编辑那一行，零新机制）。**不做节点级覆盖**：权限面回答「这个阶段在干什么」，
拆到 `validate_input` / `execute` / `validate_output` 三个工位只多出错面、且没人会去配。

**默认值**：真实阶段 `auto`（**等于今天的现状**——这是「落地不改变任何已运行行为」的兑现），
`foreman` 行 `ask`（开箱姿态不是最松的一档）。

**验证**：非法档位字符串在写入时拒——照 `SkillMode::parse` 的既有姿态（未知字面量是校验错误，
`agent/skills.rs:65-72`），前端 `StageConfigForm` 的下拉与它同步。

## 3. 值班长的域与补偿（决策 206）

域 = `home.root()`（用户裁决）。两条补偿：

1. **`FileToolPolicy` 按路径前缀**把 `{root}/data` 与 `{root}/logs` 加进 deny。今天 `deny_paths`
   是 `Vec<String>`（`file_policy.rs:21-38`）而默认名单是模式（`.env*` / `*.pem` / `id_rsa*` /
   `~/.ssh`，`file_policy.rs:46-55`）；`data/agentpipeline.db` 明文存 provider 密钥（决策 112），
   模式名单盖不住它，故用路径前缀。落地时确认 `deny_paths` 的匹配语义（前缀还是精确）并按需补齐。
2. **`foreman` 行的默认档位 `ask`**。

**登记的残余风险，无补偿**：`auto` 档下值班长可经 `cat data/agentpipeline.db` 读走全部 provider
明文密钥。`FileToolPolicy` 不管命令；`cwd` 域对命令也**几乎无约束**（命令自己 `cd` 就出去了）——
所以「给它一个空工作目录」这条路对命令不成立，这也是域最终选 `home.root()` 之后再无第二种补偿的
原因。唯一根本解是 OS 级沙箱（`docs/operations.md:1226`）。票 04 负责把这条写进
`docs/operations.md` 的残余风险表（今天那一行只写流水线），并在决策 182⑤ 当初排除 `run_command`
的理由上标注「该理由仍成立，由用户知悉后接受」。

## 4. 与 188 的关系

- `ask` 档的载体就是 188 的提议表与确认钮（票 02 依赖 `foreman-capabilities` 的 02 / 03）。
- **E 层不使用提议表做日志**：`auto` 档直接执行并落 `kanban_node_commands`（会话归属由迁移 0012 给），
  只有 `ask` 档才生成提议。提议表因此不管日志，「提议」与「审计」两件事不混。
- `ask` 档**只留给值班长**：流水线只有 `auto` / `deny` 两档（决策 206）。无人值守的线上一颗要人等
  命令的钮等于把整条线挂起，而 HITL 的价值在对讲台——那里的确认钮已经存在。

## 5. 明确不做

- **不做运行时切换**：对讲台没有地方放它（决策 198 锁死顶栏三项与 138px 定值、决策 192 的窄屏只有
  两个钉住物、页头一涨就吃对话区）。模式是配置，改完立即生效，照决策 187「保存即生效」的先例。
- **不做第四档**「白名单内自动」（见 §2）。
- **不给流水线阶段开 `ask`**（见 §4）。
- **不做 OS 级沙箱**：它登记在 `docs/operations.md:1226`，属后续决策，不进本批。
- **不做按命令的二进制白名单**：那是第四档的变体，边界该由档位与出口闸各管各的。

## 6. 影响面（票拆分的依据）

| 面 | 今天 | 本批落地后 |
|---|---|---|
| `stage_configs` | 无档位列 | 新列 + 全局默认两层（票 01） |
| `ToolExecutor::execute` | 两道闸（名单、出口） | 第三道：档位分叉（票 02） |
| `kanban_node_commands` | `task_id NOT NULL` + 外键 | 可空 + `session_id`（**在 `foreman-sessions` 01 的迁移里**，不是本批） |
| 值班长的 `FileToolPolicy` | `vec![home.root()]`，deny 是模式 | 加 `data` / `logs` 路径前缀（票 03） |
| 值班长的命令卸载 | 会落到 `{root}/tasks/.context`（空 task_id） | 落会话维度，且守住「空 task_id 不写 `tasks/.context`」（票 03） |
| `docs/operations.md:1179` | 残余风险只写流水线 | 扩到对讲台 + 标注无补偿（票 04） |
