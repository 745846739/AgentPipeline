# Agent Prompt 设计

> 拆分自 agent-pipeline.md（原 §10）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 10. Agent Prompt 设计

### 10.1 核心原则

1. **文件与元数据分离。** agent 通过 `write_file` tool 写入产出文件，通过 `submit_metadata` tool 返回结构化元数据。文件内容不进 state，元数据用于流转判断。
2. **每个节点独立对话。** 每次 agent 调用使用独立的对话上下文，主 history 只保留最终结果。
3. **validate_input / validate_output 复用同一个 agent，prompt 不同。** 例外：`develop` / `test` 的 validate_output 为纯代码，不调用 agent（决策 62）。
4. **AGENTS.md 注入位置固定。** 拼入 system prompt，完整段落顺序为 `[基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]`（工作目录为 G12 的绝对路径段；技能清单承载技能正文，决策 170）；AGENTS.md 不存在时注入非空默认上下文（项目根路径 + 语言/测试框架 + "本仓库无 AGENTS.md"）。固定前缀保证 prompt cache 稳定命中（§12.13.5）。

### 10.2 Tool 定义

```python
tools = [
    {
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "将产出内容写入指定文件路径。覆盖写入，保证幂等。",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "相对于任务目录或 worktree 的文件路径"},
                    "content": {"type": "string", "description": "文件内容"}
                },
                "required": ["path", "content"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "edit_file",
            "description": "编辑现有文件的部分内容。精确替换指定文本，保证幂等。",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "文件路径"},
                    "old_text": {"type": "string", "description": "要替换的原文"},
                    "new_text": {"type": "string", "description": "替换后的内容"}
                },
                "required": ["path", "old_text", "new_text"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "读取指定文件的内容。支持 offset 和 limit 参数分段读取。",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "文件路径"},
                    "offset": {"type": "integer", "description": "起始行号（从 0 开始）"},
                    "limit": {"type": "integer", "description": "读取行数"}
                },
                "required": ["path"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "delete_file",
            "description": "删除指定文件。对应 FileChangeSpec.action = \"delete\"，保证幂等（文件不存在时视为成功）。",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "要删除的文件路径"}
                },
                "required": ["path"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "list_dir",
            "description": "列出目录内容。最多返回 200 项，超出折叠为目录摘要。",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "目录路径"},
                    "recursive": {"type": "boolean", "description": "是否递归列出，默认 false"}
                },
                "required": ["path"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "run_command",
            "description": "执行 shell 命令。cwd 默认为 worktree（仅作卫生默认值，不构成安全边界）。",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "要执行的命令"},
                    "cwd": {"type": "string", "description": "工作目录（默认 worktree）"},
                    "timeout_sec": {"type": "integer", "description": "超时秒数；缺省时按阶段决定上限（决策 75）"}
                },
                "required": ["command"]
            }
        }
    },
    {
        "type": "function",
        "function": {
            "name": "submit_metadata",
            "description": "提交本轮执行的结构化元数据，用于流转判断。parameters 由各阶段的 Rust serde 结构体经 schemars 派生（决策 38），下例为 architect-design.execute 的形态。",
            "parameters": {
                "type": "object",
                "properties": {
                    "readiness": {"type": "boolean", "description": "产出是否充分"},
                    "blockers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "阻塞项列表（readiness=false 时必填）"
                    },
                },
                "required": ["readiness"]
            }
        }
    }
]
```

> **阶段私有字段：** `submit_metadata` 的 `parameters` 不是全局固定的，而是按 `(stage, node)` 用对应的 Rust 结构体派生。例如 architect-design.execute 额外暴露 `affected_files`、`new_symbols`、`conflict_warnings`；test.execute 暴露 `failures[].failure_cause`。tool 定义与校验逻辑同源，不会漂移。

> **`run_command` 超时上限（决策 75）：** `tool_timeout_sec`（默认 60）是**默认值**而非所有命令的上限。`test.execute` 需要由 agent 通过 `run_command` 跑集成测试，60 秒显然不够。规则：agent 显式传 `timeout_sec` 时取该值；未传时，test / merge 阶段取 `test_command_timeout_sec`（默认 600），其余阶段取 `tool_timeout_sec`。系统驱动的测试命令始终用 `test_command_timeout_sec`（决策 66）。

> **`spawn_sub_agent`（扩展工具，默认关闭）：** 子代理用于上下文超限兜底（§12.8 / §12.13 L4）。默认不启用；在阶段配置中开启后，agent 可派生一层子代理。关闭时 §12.13 L4 兜底直接跳到 `pending(context_overflow)`。

### 10.3 各节点 Prompt 模板

#### architect-design

**validate_input：**

```python
system = """
你是架构设计的信息充分性检查 agent。判断任务信息是否足够进行架构设计。

## 判断标准
- 有明确的功能需求描述
- 有基本的技术约束（语言、框架、兼容性等）
- 有可识别的输入输出定义

## 输出
调用 submit_metadata 返回检查结果。
- readiness: boolean（信息是否充分）
- blockers: string[]（不充分时列出缺失项）
"""

user = """
任务标题：{task_title}
任务描述：{task_description}
"""
```

**execute：**

```python
system = """
你是架构设计 agent。根据用户需求生成设计文档。

## 输出步骤
1. 分析需求，撰写设计文档
2. 调用 write_file 将文档写入 design.md
3. 调用 submit_metadata 返回元数据

## 设计文档格式（design.md）
# {task_title}
## 需求概述
## 技术方案
## 涉及文件
| 文件路径 | 改动类型 | 说明 |
## 验收标准
- AC-1: {可验收的完成判据}
- AC-2: ...
## 风险点

## submit_metadata 字段（architect-design.execute）
- affected_files: 涉及的源码文件路径列表
- new_symbols: 本次新增的公开符号列表 [{name, kind, module_path, file_path}]
- conflict_warnings: 文件/符号重叠警告
- acceptance_criteria: [{id, description}] 验收标准清单（与 design.md「验收标准」节一一对应，决策 136；下游 test-design 的场景经 design_refs 引用、sync-check 机械校验引用完整性、review 逐条对照）
"""

user = """
任务标题：{task_title}
任务描述：{task_description}
"""
```

> **backtrack / 重规划重入的反馈注入（决策 126 / 138）：** sync-check backtrack 时把双方 blockers 写入任务目录 `backtrack-feedback.md`（与游标归档同一事务）；develop / test 的 `retry_exhausted` 经"带失败摘要回架构设计"（决策 138）重入时，系统在同一事务内把重试历史摘要写入 `retry-feedback.md`。重入后：execute 的 user prompt 追加「上游反馈」段（`backtrack-feedback.md` 与 `retry-feedback.md` 的内容，注明来源与诉求——上一轮设计不足以支撑开发/测试，或实现反复失败疑似设计问题，请针对性修订设计，含验收标准）；validate_input 的 user prompt 同步追加一行提示读取。首轮执行时两处均为空、不渲染。

**validate_output：**

```python
system = """
你是架构设计的产出质量检查 agent。验证设计文档是否充分支撑后续开发和测试。

## 检查标准
- 包含完整的技术方案
- 涉及文件列表明确
- 验收标准编号清单完整、每条可验收（决策 136）
- 数据流和模块边界清晰
- 风险点有对应措施

## 输出
1. 读取 design.md（通过 read_file）
2. 调用 submit_metadata 返回检查结果
- readiness: boolean
- blockers: string[]（不合格时列出不足之处）
"""

user = """
原始任务需求：{task_description}
设计文档路径：{design_doc_path}
"""
```

#### develop-design

**validate_input：**

```python
system = """
你是开发方案的输入充分性检查 agent。判断设计文档是否足以支撑开发。

## 判断标准
- 技术方案明确，模块划分清晰
- 涉及文件列表完整
- 数据流和接口定义明确

## 输出
调用 submit_metadata。
"""

user = """
设计文档路径：{design_doc_path}
请先读取设计文档，然后判断是否足以支撑开发。
"""
```

**execute：**

```python
system = """
你是开发方案 agent。根据设计文档输出详细开发方案。

## 输出步骤
1. 读取 design.md（通过 read_file）
2. 撰写开发方案
3. 调用 write_file 将方案写入 dev-plan.md
4. 调用 submit_metadata 返回元数据

## 开发方案格式（dev-plan.md）
# 开发方案
## 设计概要
## 实现计划
### 步骤 1: ...
## 预期文件变更
| 文件路径 | 操作 | 说明 |
## 单元测试计划

## submit_metadata 字段
- file_changes: 预期文件变更列表
"""

user = """
设计文档路径：{design_doc_path}
请先读取设计文档，然后输出开发方案。
"""
```

**validate_output：**

```python
system = """
你是开发方案的产出质量检查 agent。验证开发方案是否可执行。

## 检查标准
- 实现步骤具体可操作
- 文件变更列表完整
- 单元测试计划覆盖关键路径

## 输出
1. 读取 dev-plan.md
2. 调用 submit_metadata
"""

user = """
设计文档路径：{design_doc_path}
开发方案路径：{dev_doc_path}
"""
```

#### test-design（业务测试用例设计）

**validate_input：**

```python
system = """
你是测试设计的输入充分性检查 agent。判断设计文档是否足以支撑测试场景设计。

## 判断标准
- 有明确的功能需求和用户故事
- 有输入输出定义
- 有业务流程描述

## 输出
调用 submit_metadata。
"""

user = """
设计文档路径：{design_doc_path}
请先读取设计文档，然后判断是否足以支撑测试场景设计。
"""
```

**execute：**

```python
system = """
你是业务测试用例设计 agent。根据设计文档设计业务测试场景。

## 你只负责设计，不写代码。
产出是测试场景文档（test-scenarios.md），描述测什么、怎么测、预期结果。
集成测试代码由后续的 test 阶段编写。

## 输出步骤
1. 读取 design.md（通过 read_file）
2. 设计业务测试场景
3. 调用 write_file 将场景文档写入 test-scenarios.md
4. 调用 submit_metadata 返回元数据

## 测试场景文档格式（test-scenarios.md）
# 测试场景
## 场景 1: {场景名称}
### 前置条件
### 测试步骤
1. ...
### 预期结果
### 优先级: high/medium/low

## 场景 2: ...

## 覆盖要求
- 正常流程
- 边界条件
- 异常流程
- 权限/并发场景（如适用）
- high 优先级场景必须引用 design.md 验收标准编号（design_refs，决策 136）

## submit_metadata 字段
- test_scenarios: TestScenario[]（场景清单；每项含 design_refs: 引用的验收标准 id 列表，决策 136）
"""

user = """
设计文档路径：{design_doc_path}
请先读取设计文档，然后设计业务测试场景。
"""
```

**validate_output：**

```python
system = """
你是测试设计的产出质量检查 agent。验证测试场景文档的完整性。

## 检查标准
- 覆盖正常流程、边界条件、异常流程
- 每个场景有清晰的前置条件、步骤、预期结果
- 优先级分配合理
- high 场景的 design_refs 引用的验收标准编号真实存在（决策 136；引用悬空会被 sync-check 机械校验拦下）

## 输出
1. 读取 test-scenarios.md
2. 调用 submit_metadata
"""

user = """
设计文档路径：{design_doc_path}
测试场景文档路径：{test_scenarios_path}
"""
```

#### develop

**execute：**

```python
system = """
你是开发 agent。根据开发方案编写业务代码和单元测试。

## 输出步骤
1. 读取 dev-plan.md（通过 read_file）
2. 按方案编写业务代码
3. 编写单元测试
4. 调用 write_file 写入变更文件
5. 调用 submit_metadata 返回元数据

## 要求
- 文件写入采用"先清后写"策略
- 单元测试覆盖方案中列出的关键路径
- 若 dev-plan.md 不存在（用户跳过了开发方案阶段，决策 115），直接基于 design.md 完成开发
"""

user = """
开发方案路径：{dev_doc_path}
请先读取开发方案，然后编写代码和单元测试。
"""
```

**validate_output：** 纯代码，无 agent prompt（决策 62）。

系统按 `project.test_framework` 构建单元测试命令并执行（记录到 `kanban_node_commands`，超时用 `test_command_timeout_sec`），exit code 路由：0 → next_stage；非 0 → 重试 execute。develop 阶段的失败只有"回到 execute 修代码/用例"一条路径，无需根因分类。

#### review

**execute：**

```python
system = """
你是代码评审 agent。评审变更代码和单元测试，并对照设计文档检查实现是否符合设计。

## 输出步骤
1. 读取变更文件和单元测试文件（通过 read_file）
2. 读取设计文档与测试场景（路径由系统注入，决策 133；对应文件不存在时按决策 115 降级——本任务跳过该设计阶段，评审不含该维度）
3. 执行代码评审：逐条对照设计文档的需求概述与验收标准检查实现符合性；逐个检查单元测试断言是否真实覆盖行为（防"自写自测"的弱测试，决策 133）
4. 调用 write_file 将评审报告写入 review-report.md
5. 调用 submit_metadata 返回元数据

## 评审报告格式（review-report.md）
# 代码评审报告
## 总体评价（approved: true/false）
## 设计符合性
- AC-1: {符合 / 偏离：说明}
- ...
## 测试质量
- {测试文件}: {断言是否真实覆盖行为，指出弱断言 / 恒真用例}
## 逐文件评审
### {file_path}
- 问题：...
- 建议：...
## 必须修改项

## submit_metadata 字段
- approved: boolean
- required_changes: FileChangeSpec[]（approved=false 时；允许包含设计符合性与测试质量问题）
"""

user = """
变更文件列表：{changed_files}
单元测试文件：{unit_test_files}
设计文档：{design_doc_path}
测试场景文档：{test_scenarios_path}
请逐一读取并评审。
"""
```

#### test（写集成测试 + 执行）

**execute：**

```python
system = """
你是测试 agent。根据测试场景文档编写集成测试代码并执行。

## 输出步骤
1. 读取 test-scenarios.md（通过 read_file）
2. 读取变更的业务代码文件（通过 read_file），了解实现细节
3. 根据测试场景编写集成测试代码，按项目测试框架惯例放置（路径约定：{test_file_convention}）
4. 执行集成测试（通过 run_command，命令：{test_command}）
5. 调用 write_file 将测试报告写入 test-report.md
6. 调用 submit_metadata 返回元数据

## 集成测试要求
- 每个测试场景对应至少一个测试用例
- 使用目标项目的测试框架（{test_framework}）
- 文件命名遵循 {test_file_convention}
- 包含必要的 setup / fixture 和 mock
- 用例本身有编译/格式问题时先修复再执行
- 测试用例是硬产出，必须全部通过
- 若 test-scenarios.md 不存在（用户跳过了测试设计阶段，决策 115），根据 design.md 自行设计场景后再编写测试

## 测试报告格式（test-report.md）
# 集成测试报告
## 执行结果
## 失败用例详情
### {test_name}
- 错误信息：...
- 根因分类：test_issue（用例问题） / code_issue（业务代码问题）
## 结论（passed: true/false）

## submit_metadata 字段
- passed: boolean
- failures: [{test_name, error_message, failure_cause}]（failure_cause ∈ test_issue | code_issue）
"""

user = """
测试场景文档路径：{test_scenarios_path}
变更的业务代码文件：{changed_files}
测试框架：{test_framework}
测试命令：{test_command}
请先读取测试场景和代码，然后编写集成测试并执行。
"""

# gate_recheck = true 时（被 merge 测试闸门打回后的复检，决策 85 / 109）追加：
user += """
## 本次为合入前测试闸门的复检
上一次 merge 阶段的合入闸门（单元 + 集成测试）失败，失败输出如下：
{gate_failure_output}
（完整日志见 kanban_node_commands，可按 stage=merge 过滤）

请重新分析每个失败用例的根因，并更新 failure_cause：
- 失败源于用例本身（如上游新代码导致断言过时）→ test_issue，请直接修正用例
- 失败源于业务代码 → code_issue，交由用户决定改用例还是回开发
不要为了通过而弱化断言。
"""
```

**validate_output：** 纯代码，无 agent prompt（决策 62）。

读 `test_result.passed` 与 `failures[].failure_cause` 路由：passed=true → next_stage；全部 `test_issue` → 重试 execute（修用例）；存在 `code_issue` → pending(user_decision)，用户选择 goto execute 或 goto develop.execute。

### 10.4 节点级重试模型

**核心：每次 agent 调用使用独立对话，不污染主对话 history。**

> **上下文超限处理见 §12.13。** 节点内 agent loop 可能因多轮 tool 调用累积超限，通过四级压缩策略（裁剪 → 卸载 → 压缩 → 兜底）处理，不依赖节点级重试解决。

```python
async def run_node_with_retry(stage, node_type, user_prompt, max_retries=agent_retry_max):
    # max_retries = agent_retry_max（决策 33）：
    # 覆盖 loop 整体失败——元数据解析/校验失败、空闲/绝对超时、agent 崩溃
    last_error = None
    system_prompt = build_system_prompt(stage, node_type)  # [基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]

    for attempt in range(max_retries):
        messages = [
            {
                "role": "system",
                "content": [
                    {"type": "text", "text": system_prompt, "cache_control": {"type": "ephemeral"}}
                ],
            },
            {"role": "user", "content": user_prompt},
        ]

        try:
            # agent loop：内部工具失败按 tool_retry_max 重试（G13），返回最终回复或抛整体失败
            response = await run_agent_loop(
                messages, tools=stage_tools,
                idle_timeout=effective_idle_timeout(stage, node_type),
                max_duration=effective_max_duration(stage, node_type),
            )
        except NodeTimeout:
            last_error = "节点执行超时"
            continue  # 干净对话重来

        metadata = extract_metadata(response)
        valid, errors = validate_metadata(metadata, metadata_struct_for(stage, node_type))
        if valid:
            return NodeResult(success=True, metadata=metadata)

        last_error = "; ".join(errors)
        user_prompt = f"{user_prompt}\n\n上次调用失败：{last_error}\n请重新调用 submit_metadata。"

    return NodeResult(success=False, error=last_error)
```

**超时配置取值：** `effective_idle_timeout` / `effective_max_duration` 的有效值 = 节点级覆盖 > 阶段级覆盖 > 全局默认（决策 66）。超时触发时杀死整个进程组（`tokio::process::Command` 的进程组），避免 cargo / pytest 子进程残留。

> **伪阶段调用的心跳归属（决策 88 / 100）:** 伪阶段是在某个正式节点的执行过程中**同步**发起的第二个 LLM 调用（`conflict_check` 在 `architect-design.execute` 内，决策 60/67）。它的流式 token、工具活动必须计入**父节点的心跳**——写入同一条 `kanban_node_runs` 的 `last_activity_at`，使用同一 `process_group_id`。否则 `node_idle_timeout_sec`（默认 300s）会在比对期间把 architect-design.execute 误判为超时并杀掉。同时给 `conflict_check` 设一个较短的阶段级 `max_duration_sec`，避免它把父节点拖到 `node_max_duration_sec` 上限。**但观测上伪阶段有自己独立的 run 行与会话行**（`agent_type = "pseudo:conflict_check"`，`parent_run_id` 指向父 run，`cursor_id` 继承父游标），因此用户能看到它为什么判定 `duplicate_risk`；计量不重复计入——`kanban_tasks.total_tokens` = 所有 run 行求和，父 run 的 `prompt_tokens` **不含**子行。

> **复判伪阶段的心跳归属（决策 134）：** `validator_cross_check` 在 agent 型 validate_output 节点内**同步**发起（首判不合格时），与 conflict_check 同模式：流式 token 与工具活动计入父节点心跳（同一 `kanban_node_runs.last_activity_at` 与 `process_group_id`），观测上有独立 run / 会话行（`agent_type = "pseudo:validator_cross_check"`，`parent_run_id` 指向 validate_output 的 run，`cursor_id` 继承父游标），用户可对照查看首判与复判两侧结论；其失败视为父节点失败，不触发独立节点级重试。

> **系统命令也要刷新心跳（决策 100）：** merge 阶段的合入闸门要跑单元 + 集成测试，最长可到 `test_command_timeout_sec`（默认 600s），超过 `node_idle_timeout_sec`（默认 300s）。因此 `run_recorded_command`（§12.4.4）在命令**开始与结束**时都要刷新所属 run 的 `last_activity_at`，长命令执行期间也按输出行周期刷新。否则闸门会被空闲超时误判。

**对话历史对比：**

| 方案 | 重试 3 次后消息数 | context 占用 |
|---|---|---|
| 单对话内重试 | 8 条 | 高，堆积无用消息 |
| 节点级独立对话 | 最终 2-3 条 | 低，每次干净 |

**LLM Cache 策略：** system prompt 用 `cache_control` 标记缓存断点。重试时 system 不变命中缓存，user 部分短（200-500 token）损失小。

### 10.5 System Prompt 管理

```
~/.agentpipeline/prompts/
├── architect_design/
│   ├── validate_input.md
│   ├── execute.md
│   └── validate_output.md
├── develop_design/
│   ├── validate_input.md
│   ├── execute.md
│   └── validate_output.md
├── test_design/
│   ├── validate_input.md
│   ├── execute.md
│   └── validate_output.md
├── develop/
│   └── execute.md              # validate_output 为纯代码，无 prompt
├── review/
│   └── execute.md
├── test/
│   └── execute.md              # validate_output 为纯代码，无 prompt
├── project_analysis/           # 伪阶段：项目静态分析（决策 48）
│   └── analyze.md
├── conflict_check/             # 伪阶段：语义冲突比对（决策 67）
│   └── compare.md
├── validator_cross_check/      # 伪阶段：validate_output 异族复判（决策 134）
│   └── judge.md
└── common/
    ├── tool_usage.md
    └── format_rules.md
```

> **伪阶段说明：** `project_analysis`、`conflict_check` 与 `validator_cross_check` 不进入 kanban 图，没有 StageIO / checkpoint / pending，仅复用阶段配置机制（provider / model / prompt / 白名单校验）。

### 10.6 阶段级 Agent 配置

> 全局参数表（validate / 重试 / 超时 / 并发等）见 [overview.md](overview.md) §3；本节只涉及阶段级覆盖。

每个阶段可独立配置 **provider / persona / tools / skills / mcp**，但**不能削减系统最小基线**——只能在其之上增量扩展。

#### 10.6.1 配置分层

```
┌─────────────────────────────────────────────┐
│  L1 系统基线（System Baseline）              │  不可覆盖，只能继承
│  - 强制 context / skills                    │
│  - provider 白名单                           │
│  - 禁止工具、安全约束（沙箱）                 │
│  - 计量与审计（强制）                         │
└───────────────────┬─────────────────────────┘
                    │ 继承
┌───────────────────▼─────────────────────────┐
│  L2 阶段默认配置（Stage Config）              │  可覆盖
│  - provider / model                          │
│  - persona（system prompt）                  │
│  - tools / skills（增量）                    │
└───────────────────┬─────────────────────────┘
                    │ 继承
┌───────────────────▼─────────────────────────┐
│  L3 局部覆盖（Node Override + 任务覆盖）      │  可覆盖
│  - validate 节点可用更便宜的模型               │
│  - task.model_override 本任务整体换 provider  │
└───────────────────┬─────────────────────────┘
                    │
┌───────────────────▼─────────────────────────┐
│  L4 运行时约束（Runtime）                     │  动态
│  - 上下文压缩、超时                           │
└─────────────────────────────────────────────┘
```

> **provider 解析优先级（决策 129）：** `node_overrides > task.model_override > 阶段 provider > 全局默认`。任务级覆盖（决策 105 的 `Task.model_override`）插在节点覆盖与阶段配置之间——节点覆盖是更局部的意图；任务覆盖是运行时整体替换，不改写任何 `stage_configs` 行。

#### 10.6.2 系统最小基线（不可覆盖）

```typescript
interface SystemBaseline {
  // ── 强制注入，阶段无法移除 ──
  mandatory_context: string[];      // 如 ["AGENTS.md"]
  mandatory_skills: string[];       // 默认空 []，全部走用户配置；skill 不存在时 fail fast（决策 47）
  mandatory_tools: string[];        // 如 ["submit_metadata", "read_file", "write_file", "edit_file", "delete_file", "list_dir", "run_command"]

  // ── 厂商能力边界（决策 103） ──
  // 注意这是"代码支持哪些适配器"，不是"用户启用了哪些 provider"。
  // 用户启用的集合存在 DB 的 providers 表（§11.5），校验 = 阶段 provider ∈ supported_adapters ∩ enabled。
  supported_adapters: string[];     // openai | anthropic | deepseek | ... 硬编码常量，改它要发版

  // ── 硬约束 ──
  forbidden_tools: string[];        // 禁止工具（如危险 shell 操作）

  // 文件工具路径策略（决策 104）。**只作用于文件工具，不是系统级沙箱**：
  // OS 层面不做任何 confinement，run_command 的 shell 不受限（决策 19 已修订）。
  file_tool_policy: {
    workdir_bound: string[];        // 文件工具的允许根：worktree + 任务目录
    deny_paths: string[];           // 禁止文件工具访问的路径（默认：.env*、*.pem、*.key、id_rsa*、~/.ssh）
    resolve_realpath: true;         // 判定前对目标路径做 realpath 解析（macOS 上 /etc → /private/etc，
                                    // /tmp → /private/tmp）；不做解析会让 deny 静默失效
    refuse_symlink_write: true;     // 拒绝写符号链接；已存在符号链接按解析后的目标判定，堵住逃逸
  };

  // ── 强制机制 ──
  metering: true;                   // token 计量不可关闭
  audit_log: true;                  // 会话落库不可关闭
  structured_output: true;          // 结构化输出校验不可关闭
}
```

**最小基线包含：** AGENTS.md 加载（G3）、工作目录告知（G12）、结构化输出校验（§12.12）、token 计量（§12.2）、会话审计（§12.4.3）、**文件工具路径策略**（`file_tool_policy`，决策 104）。

**技能（决策 170，修订决策 47）：** `mandatory_skills` 默认空。技能的**名字是唯一身份**，三类来源：

| 来源 | 判定 | 是否携带正文 |
|---|---|---|
| 内嵌默认 | 二进制内 `EMBEDDED_SKILLS`（`crates/core/src/agent/skills.rs`；决策 7 的内嵌 persona 先例） | 有 |
| 用户 markdown | `~/.agentpipeline/skills/{name}/SKILL.md`（镜像 ZCode 布局，可直接拷贝；**同名覆盖内嵌**） | 有 |
| PATH 外部工具 | PATH 中的可执行文件（决策 47 原语义，如 `rtk` / `codegraph`） | 无，只列名字 |

**知识型技能的正文注入 system prompt**（`## 已启用技能` 段：工具型渲染为 `- {name}`，知识型渲染为 `### {name}` + 正文），因此 `prompt_template_hash` 对正文敏感（决策 137）。正文「存在且非空」在启动与 `PUT /stage-configs` 时 **fail fast**（与 §10.6.4 的 `persona_path` 同口径）；工具型技能未安装却被引用时同样 fail fast，与 MCP 处理一致。

**节点级技能：** 阶段级 `skills_json` 无法区分节点，而同一阶段的不同节点职责可能互斥（architect-design 的 `validate_input` 提问、`execute` 写 `design.md`、`validate_output` 校验）。因此技能也可在 `node_overrides_json[node].skills` 声明——见 §10.6.3。有效集 = `mandatory_skills ∪ 阶段级 skills_json ∪ 节点级 skills`（**只增不减**）。

**约束语义：** 阶段配置只能让 agent **能力更强或更聚焦**，不能让 agent **绕过系统保障**。例如阶段可以增加工具，但不能移除 `submit_metadata`；可以选择白名单内的 provider，但不能自选白名单外的。

#### 10.6.3 阶段可配置项

```typescript
interface StageAgentConfig {
  stage: string;

  // ── provider ──
  provider?: {
    provider_id: string;           // 引用 DB providers 表的一行（决策 111）——阶段不再单独存 model
    temperature?: number;
    max_tokens?: number;
    response_format?: object;      // JSON 模式参数（§12.12）
  };

  // ── persona ──
  persona?: {
    system_prompt_path: string;    // 如 "prompts/architect_design/execute.md"
    append?: string;               // 追加的额外指令
    // 基线会在此基础上追加强制前言（AGENTS.md 加载、安全规则）
  };

  // ── 能力（增量） ──
  tools?: string[];                // 声明的工具（与基线取并集）
  skills?: string[];               // 加载的 skill（与基线取并集；可含知识型技能，其正文注入 prompt，决策 170）
  mcp_servers?: string[];          // 启用的 MCP（与基线取并集）

  // ── 节点级覆盖 ──
  node_overrides?: {
    [node: string]: Partial<StageAgentConfig> & {
      idle_timeout_sec?: number;   // 覆盖全局 node_idle_timeout_sec（决策 66）
      max_duration_sec?: number;   // 覆盖全局 node_max_duration_sec
      skills?: string[];           // 该节点专属技能（与阶段级取并集，决策 170）
    };
  };
}
```

> **节点级 `skills` 的存在理由（决策 170）：** 阶段级 `skills` 是整阶段生效的，而同一阶段的节点职责可能互斥。典型用例——architect-design 的 `validate_input` 需要「拷问」（`grilling`：把设计树走到没有悬空分支、只把决定问用户），`execute` 需要「综合成规格」（`to-spec`：不再提问、把已定内容写成 `design.md`）。若只在阶段级声明，写文件的节点也会拿到「不断向用户提问」的指引，二者只能互相打架。

#### 10.6.4 合并与校验规则

| 配置项 | 合并方式 | 校验 |
|---|---|---|
| provider | 阶段引用 `provider_id`（无则用系统默认） | 该行的厂商必须 ∈ `supported_adapters` **且** 该行 `enabled = 1`，否则**加载时拒绝**（决策 103 / 111） |
| provider（运行时解析） | `node_overrides > task.model_override > 阶段 provider > 全局默认`（决策 129） | `model_override` 引用的 provider_id 在设置时（`POST /tasks/{id}/model-override`）须过同一校验 |
| persona | 阶段 prompt + 基线强制前言 | 必须存在且非空 |
| tools | `基线 mandatory_tools ∪ 阶段 tools − forbidden_tools` | 不能移除 mandatory_tools |
| skills | `基线 mandatory_skills ∪ 阶段 skills ∪ 节点级 skills`（决策 170，只增不减） | 不能移除 mandatory_skills；引用的 skill 名字必须存在，否则 fail fast；知识型技能的**正文必须存在且非空**，否则 fail fast（与 `persona_path` 同口径） |

**校验时机：** 启动时（配置加载）一次性校验所有**注册阶段**的阶段配置，**fail fast**——发现违规配置直接拒绝启动并报错，不允许运行时才暴露。伪阶段（`project_analysis` / `conflict_check` / `validator_cross_check`）按各自要求单独校验（决策 87 / 134）：`project_analysis` 的 persona **允许为空**（省略时只输出确定性探测的事实清单，决策 78）；`conflict_check` 必须做语义比对，persona **强制存在且非空**；`validator_cross_check` persona 同样**强制存在且非空**，且 `cross_family_judge = true` 时必须已配置 provider，否则配置加载 fail fast。其余校验（厂商适配器支持、工具并集、超时覆盖）与正式阶段完全一致。

**DB 中不受支持的 provider 行（决策 103）：** 启动时遍历 `providers` 表，若某行的厂商 ∉ `supported_adapters`（例如升级后适配器被移除，或手工改库），**不崩溃**——把该行 `enabled` 置 0 并在 UI 告警；只有当某个 `stage_configs` 仍引用它时，配置加载才 fail fast，报"不支持的厂商，需代码适配"。这是**非对称处理**：坏数据降级、被引用的坏数据拒绝启动。

```python
def validate_stage_config(cfg: StageAgentConfig, baseline: SystemBaseline, providers: list[ProviderRow]):
    errors = []
    if cfg.provider:
        row = providers.get(cfg.provider.provider_id)
        if row is None:
            errors.append(f"{cfg.stage}: 引用的 provider_id '{cfg.provider.provider_id}' 不存在")
        elif row.vendor not in baseline.supported_adapters:
            errors.append(f"{cfg.stage}: 不支持的厂商 '{row.vendor}'，需代码适配")
        elif not row.enabled:
            errors.append(f"{cfg.stage}: provider '{row.id}' 未启用")

    effective_tools = (set(baseline.mandatory_tools) | set(cfg.tools or [])) - set(baseline.forbidden_tools)
    missing = set(baseline.mandatory_tools) - effective_tools
    if missing:
        errors.append(f"{cfg.stage}: 缺少强制工具 {missing}")

    # skills / mcp 同理
    if errors:
        raise ConfigError("\n".join(errors))
```

#### 10.6.5 配置示例

```toml
# ~/.agentpipeline/config.toml
# 注意：不含 provider / model / API key 等敏感或界面可改的配置（决策 22、46、56）

[server]
host = "127.0.0.1"
port = 8788

[pipeline]
validate_retry_max = 3
agent_retry_max = 3
tool_retry_max = 3
node_idle_timeout_sec = 300
node_max_duration_sec = 1800
tool_timeout_sec = 60
test_command_timeout_sec = 600
max_concurrent_tasks = 5
semantic_conflict_check = true
cross_family_judge = false
allow_dirty_worktree_merge = false

[logging]
level = "info"                       # EnvFilter 表达式；非法值回退 info，不阻断启动
format = "pretty"                    # pretty（缺省）| compact | json
file = "~/.agentpipeline/logs/agentpipeline.log"
# 不写 file 只输出到标准输出。file 的 ~ 会展开、相对路径按 home 根解析；
# 目录自动创建为 0700、文件 0600（§12.14）。文件打不开时降级为仅标准输出，不阻断启动。
# 已废弃：旧键 json_file（bool）。json_file = true 等价 format = "json"。
# 两者同时配置属冲突 → 配置加载 fail fast（决策 47 / 103 / 134 姿态）。

[prompts]
dir = "~/.agentpipeline/prompts"     # 覆盖 prompt 模板目录；缺省回落 {home}/prompts
```

> **配置校验姿态（票 16）：** `config.toml` 中未知的 section / 键一律**拒绝启动**
> （`deny_unknown_fields` 施加于 `Config` / `ServerConfig` / `PipelineOverrides` /
> `LoggingConfig` / `PromptsConfig`），不静默忽略——与决策 47 / 103 / 134 的 fail fast 姿态一致。
> `[logging]` 的 `format` 与已废弃 `json_file` 同时出现同样报错。
>
> **升级注意（行为变化）：** 此前拼错或多余的键会被静默忽略、按默认值运行；现在**启动即报错**。
> 这是有意的收紧——静默忽略会让「配置写了却没生效」无从察觉。

**阶段级 Agent 配置**存储在 SQLite 数据库中，通过前端界面配置。每个阶段可独立设置 provider（引用 `providers` 表的 `provider_id`）、tools、skills、超时覆盖。系统最小基线（mandatory_tools、mandatory_skills、`file_tool_policy`）在代码中硬编码，不可覆盖。模型上下文窗口随 `providers` 表的一行存在一起（决策 46 / 111）——**阶段不单独存 model**，换模型即换 `provider_id`，这样 L0 容量预估（§12.13.3）查找窗口大小的路径唯一。伪阶段（`project_analysis` / `conflict_check` / `validator_cross_check`）复用同一配置机制（决策 67 / 87 / 134）；`cross_family_judge = true` 时 `validator_cross_check` 必须已配置 provider，否则配置加载 fail fast。

**节点级技能配置示例（决策 170）——给 architect-design 配「拷问 + 综合成规格」：** 内嵌技能 `grilling` / `to-spec` 开箱可用，无需先放文件；在 `node_overrides_json` 里按节点声明即可（`PUT /stage-configs/architect-design` 整条替换该阶段配置）：

```json
{
  "validate_input": { "skills": ["grilling"] },
  "execute":        { "skills": ["to-spec"] }
}
```

`validate_input` 因此拿到「把设计树走到没有悬空分支、只把**决定**问用户（事实自己查）、经 `submit_metadata.blockers` 提问」的指引；`execute` 拿到「不再提问、把已定内容综合成 `design.md`（保留 §10.3 必需节与验收标准编号清单）」的指引。想用自己版本的技能，把文件放到 `~/.agentpipeline/skills/grilling/SKILL.md` 即覆盖内嵌（同名覆盖，正文进 prompt）。回滚：`DELETE /stage-configs/architect-design` 撤销该阶段覆盖，行为回到内嵌默认。

**首启引导：** 未配置任何 provider / API key 时，创建任务返回明确错误（提示先配置 provider），不使用隐式默认模型（决策 56）。

#### 10.6.6 模型分级策略

validate 节点是**判断型任务**（读产出 → 返回 readiness + blockers），execute 节点是**生成型任务**，需要强模型。**校验比生成更难做对**：生成错了会被拦下重试，校验漏检就是静默放行、错误流向下游（决策 134 的动机）——因此校验档位不应低于生成档位，且**推荐与同阶段 execute 使用不同 vendor** 的模型，避免同源偏差（self-preference bias：judge 偏爱自己风格的产出）。

| 节点类型 | 推荐模型档位 | 理由 |
|---|---|---|
| validate_input / validate_output（agent 型） | 与同阶段 execute **不同 vendor** 的模型，档位同级或更高（决策 134） | 判断题输出短，但漏检代价是错误放大；异族配置消同源偏差 |
| execute（architect / develop / test） | 强模型 | 生成质量直接决定产出质量 |
| review.execute | 强模型 | 需要发现深层问题 |
| validator_cross_check（伪阶段，决策 134） | 强档（与 review.execute 同级） | 复判是裁决性判断，全流程最难的判断之一 |
| sync-check | 不调 LLM | 纯代码逻辑 |
| develop / test 的 validate_output | 不调 LLM | 纯代码执行测试 + 路由（决策 62） |
| project_analysis / conflict_check（伪阶段） | 便宜模型（决策 67） | 结构化分析，不需要强生成能力 |

**注意：** 换模型会改变 prompt cache 的命中（不同模型缓存独立），节点级覆盖时应保持同一节点的模型稳定，不要在同一节点的多次重试间切换模型。

#### 10.6.7 MCP 接入（v2 预留）

v1 不实现 MCP；完整设计见附录 B。
