# Agent Prompt 设计

> 拆分自 agent-pipeline.md（原 §10）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 10. Agent Prompt 设计

### 10.1 核心原则

1. **文件与元数据分离。** agent 通过 `write_file` tool 写入产出文件，通过 `submit_metadata` tool 返回结构化元数据。文件内容不进 state，元数据用于流转判断。
2. **每个节点独立对话。** 每次 agent 调用使用独立的对话上下文，主 history 只保留最终结果。
3. **validate_input / validate_output 复用同一个 agent，prompt 不同。** 例外：`develop` / `test` 的 validate_output 为纯代码，不调用 agent（决策 62）。
4. **AGENTS.md 注入位置固定。** 拼入 system prompt，完整段落顺序为 `[基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]`（工作目录为 G12 的绝对路径段；技能清单承载技能正文，决策 170）；AGENTS.md 不存在时注入非空默认上下文（项目根路径 + 语言/测试框架 + "本仓库无 AGENTS.md"）。固定前缀保证 prompt cache 稳定命中（§12.13.5）。

> **本节之外的两个工具性能力**：`Skill` 工具（按名拉技能正文，决策 172③）与 `spawn_sub_agent`（只读子代理，决策 172③ / 票 08）定义在 §10.2 末尾；`run_command` 的出口策略见 §10.6.4。三者的共同点是**边界落在执行点而不是 tool 定义上**——只限制「广告出去的定义」是纸糊的，模型可以无视定义直接发一次调用。

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

> **`spawn_sub_agent`（扩展工具，需阶段显式声明）：** 派生一个**只读**子代理处理可分解的检索子任务，返回摘要——把「读 20 个文件」的原文挡在父上下文之外（决策 172③，票 08）。三条硬约束：工具集**固定为 `read_file` / `list_dir`**（无 `run_command` / 写文件 / `submit_metadata`）、**不继承阶段声明的工具**、**不再派子代理**（深度一层，决策 9）。子代理各自占一行 run（`agent_type = "subagent"` + `parent_run_id`）与其会话行，token 记在自己行上并计入任务总量，父 run 不重复累加；超时沿用节点级 `node_max_duration_sec`。
>
> **与 L4 的关系（决策 154 的边界不变）：** 这是**技能可调用的能力**，不是上下文超限兜底手段——「L4 兜底只有两级」（强制压缩 → `pending(context_overflow)`）与「分批 / 拆子代理不作为 L4 兜底」的原裁决不变。
>
> **它不在 `BUILTIN_TOOLS` 里**：内置集是「每个 agent 都可能拿到」的语义，而子代理是要显式授予的能力。未声明时父代理的工具集里根本没有它（默认关闭），不会拿到一个断腿的指针。

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

**技能（决策 170 / 172 / 185，修订决策 47）：** `mandatory_skills` 默认空。技能的**名字是唯一身份**，来源只有一个：

| 来源 | 判定 | 正文 |
|---|---|---|
| 用户 markdown | `{skills_root}/{name}/SKILL.md`（镜像 ZCode 布局，可直接拷贝；技能根默认 `~/.agentpipeline/skills`，可由 `[skills] dir` 覆盖，决策 172） | 有 |

> **内嵌技能已退场**（决策 172①，票 04）：二进制不再携带任何技能正文，两个流水线原生改写版（`grilling` / `to-spec`）随之移除。技能一律由用户从来源安装到本地，**不经二进制分发**——这同时解掉上游内容的再分发授权问题（27 个上游技能里只有 1 个带许可声明，而本仓是 MIT）。推荐默认改由配置界面承载（票 16）。
>
> **PATH 工具型技能已退场**（决策 185，修订决策 47 原语义）：一度把「PATH 里的可执行文件名」也算一种技能（只有名字、没有正文，如 `rtk` / `codegraph`），那是本系统早期把 skill 定义为「机器上装了对应的外部 CLI」时的残余，与 Agent Skills 规范（技能是**知识**）不一致。**二进制怎么用、能不能用，由 `run_command` 与系统权限决定**，与技能体系无交集。因此**引用一个不存在的技能名是启动失败**，而不是静默降级成一个没有正文的名字——老配置里声明过的 PATH 工具名（如 `rtk`）现在会在启动时失败，报错文案显式点出这一点（「技能只有 markdown 一个来源……PATH 里的可执行文件不算技能，要用它请让 agent 经 run_command 调用」）。

**安装技能的三个入口**（决策 172⑤，票 09）——全部走 `POST /skills/import`（上传 zip 原始字节）、`POST /skills/import-dir`（本地目录，支持批量）与 `GET /skills/scan?root=`（扫描一个已有技能根，如 `~/.zcode/skills`）：
- **落盘布局固定为 `{skills_root}/{name}/SKILL.md` + 兄弟文件**——兄弟文件必须一并落盘，否则票 07 的展开会缺文件；
- **校验在落盘前**：须含 `SKILL.md`、frontmatter 可解析、正文非空、`name` 与技能目录同名；坏包不落盘、不留半份痕迹；
- **同名默认拒绝**，报文报出冲突技能名与它**当前的来源**（技能根下那份 `SKILL.md` 的路径），`overwrite=true` 才覆盖（整目录替换，旧兄弟文件不留残骸）；
- **路径穿越按两道独立判定拒绝**（`..` / 绝对路径 / 反斜杠伪装），落盘前再实数校验目标在技能根之内；
- **卸载不检查引用**：`DELETE /skills/{name}` 允许删掉仍被配置引用的技能，引用完整性由启动校验与 `PUT /stage-configs` 的 fail fast 兜住（技能名是唯一身份，不得静默降级）。`GET /skills` 的 `declared_in` 字段负责在动手前告知后果。- **全程离线**：本组端点不依赖任何网络。

**从远程 registry 安装**（决策 172⑤，票 10；来源可在界面上改，决策 187）——`GET /market/search?q=` 查候选，`POST /market/install {name, overwrite}` 下载并安装：
来源白名单原先只能写 `config.toml` 的 `[market] allowed_sources`（改完要重启，界面上无处可改，故用 `GET /market/config` 读、`PUT /market/config` 保存——**保存即生效**（当场换客户端）、`DELETE /market/config` 回到配置文件那一级；界面在「设置 · 技能市场」（决策 187）。校验与配置解析共用 `config::validate_market_sources` 一个函数：放行一个来源等于允许从它下载引导 agent 的正文，这条判定不能有第二个版本。

- **索引格式**（本票定义）：`GET {source}/index.json`，`{"skills": [...]}`，每条含 `name` / `version` / `sha256` / `source` / `description` / `url` 六字段。`url` 允许与索引**不同源**（CDN 常见），正因如此安装时会**单独校验 `url` 的 origin 也在白名单内**；
- **来源白名单**：`config.toml` 的 `[market] allowed_sources`（见 §10.6.5）或**界面上的「设置 · 技能市场」页**（决策 187，那份优先、保存即生效），**默认空 = 不允许任何远程安装**。照 Claude Code `strictKnownMarketplaces` / Codex `allowed_sources` 的姿态——忘配的代价是装不上（用户立刻发现），配宽的代价是静默装上陌生来源。判定按 **origin**（`scheme://host[:port]`），不接受带路径的写法（那会让用户以为放行了一个前缀）。**非回环来源必须 `https`**（回环放行 `http`，便于本机起 registry 开发）——明文 http 下 `sha256` 挡不住中间人：攻击者可同时替换索引与包，使校验自洽通过。未放行来源的技能**连搜索候选都不进**，用户看不到装不上的东西（搜索与安装同口径：`source` 与下载地址的 origin **都**要放行）；
- **摘要校验**：`sha256` 的**权威值是从下载字节现算的**，不采信传输层声明（若采信，一个被控制的客户端可以同时改内容与声称值，校验形同虚设）。不符即拒绝安装，报文**同时给出期望值与实际值**（原始诊断进响应体的 `detail` 字段，与面向用户的 `error` 分开）；
- **不跟随 HTTP 重定向**（`Policy::none()`）——reqwest 缺省会跟最多 10 跳**跨源**跳转，而白名单判定看的是请求 URL：不关掉它，一个**已放行**的来源只要回一个 302 就能把内容指到任意别处（内网元数据端点之类），白名单当场失效。来源方若需要 CDN，应把该 origin 直接写进索引条目的 `url`（那时它会被正常校验）。core 侧另有一道**独立**判定：复检字节的**实际来源** origin；
- **下载体积有上限**（64 MiB，与本地导入端点的 `DefaultBodyLimit` 同值）——声明式 `Content-Length` 早退 + 流式累加兜底，使远程与本地两条路对内存的消耗同量级；
- **落盘复用票 09 的同一入口**（`SkillPackage::from_zip` + `install`），因此结构校验、同名冲突、路径穿越防护一处生效、两处受益——**远程包不比本地上传的包享有更宽的路**；
- **五类失败互不混淆**：`market_network`（502，下游不可达，该重试）/ `market_not_found`（404，索引里没这个技能，该换名字）/ `market_digest_mismatch` 与 `market_source_not_allowed`、`market_index_malformed`（400，拒绝安装，该改请求或配置）。混在一起的代价是用户不知道该改什么：几种动作毫无交集；
- **可测试性接缝**：市场客户端 trait（`MarketClient`）是本 effort **唯一新增的接缝**（决策 143）。生产用 `HttpMarketClient`（reqwest，复用既有 HTTP 栈，不引入第二套）；测试用 testkit 的 `FakeMarket` 提供固定索引与字节，**五条路径全部不打真网络**；
- **本机无网时票 09 不受影响**：市场失败不阻塞任何本地导入路径（有专门的契约用例钉这一点）。
- **接外部来源前先核四条**：本系统的索引格式（上一条）是**自定的**，外部市场多数不按它发布。核 ① 有没有 `{origin}/index.json` 且条目里**带摘要**；② 下载地址是不是非回环明文 `http`；③ 下载是否依赖 302 跳转；④ 包布局是不是单层 `{name}/SKILL.md`。
- **外部 registry 现状**（2026-09-16 普查：**71 个主机 × 三条索引路径**，判据是「content-type 是 JSON **且**正文能解析成 JSON」——返回 200 + `text/html` 的一律算未发布，SPA / 404 页伪装成 200 是这轮最常见的假阳性，已逐条验过正文）：**没有任何一家发布本系统的 `/index.json`（0/71）**。发布生态事实约定（下一段）的有 **7 家**，其中条目级**完全合格**（`$schema` 精确匹配 + 字段合规 + 产物现算 `sha256` 与声明摘要**逐位相符**）的 **5 家**（`agentskills.io` / `docs.firebender.com` / `docs.openhands.dev` / `docs.workshop.ai` / `mux.coder.com`），**每家只发自己那一个技能**——也就是说这个约定今天是「厂商自出版通道」，**不是「技能市场」**，别指望它一次带来一批可装的第三方技能。反面实例值得记：`www.pulumi.com` 的 15 条**全部不合格**（11 条摘要与产物不符、4 条取不到），因为它的 `url` 指向 `raw.githubusercontent.com/pulumi/agent-skills/main/…`——**`main` 是移动靶，摘要停在某个旧版本上再没更新**。
- **生态事实约定：`{origin}/.well-known/agent-skills/index.json`**（回退 `.well-known/skills/index.json`），靠 `$schema` 分版本：**v1** 无 `$schema`，条目 `{name, description, files:[相对路径]}`，**不含摘要**（能发现，不能校验）；**v2** 的 `$schema` 是 `https://schemas.agentskills.io/discovery/0.2.0/schema.json`，条目 `{name, description, type: "skill-md"|"archive", url, digest}`，`digest` 形如 `sha256:<64 位小写十六进制>`（实测与产物字节逐位相符，是真原语不是摆设）。两点现状要留意：**该 schema 的规范主机 `schemas.agentskills.io` 目前 DNS 解析不了**（`agentskills.io` 本身正常），所以这个约定处于「已实现、未正式发布」；而官方 Agent Skills 规范（`agentskills.io/specification`）只管**格式**，其「如何为 agent 加技能支持」一节明说远程场景「需要另一套发现机制——一个 API、一个远程 registry，或内置资源」，**即 registry 层是刻意不定义的**。至于成熟形态，同生态的上一层反而有正式规范可参照：Claude Code 插件市场（`.claude-plugin/marketplace.json`，源类型含 `archive` + 可选 `sha256`，白名单是 managed settings 的 `strictKnownMarketplaces`）与 **MCP Registry**（`registry.modelcontextprotocol.io/v0/servers`，版本化 schema）——但后者管的是 MCP server，技能这一层还没轮到。
- **技能识别口径：生态已收敛，我们偏窄。** 围绕 `SKILL.md` 有一条贯穿所有实现的判据——**含 `SKILL.md` 的目录就是技能，技能名 = 该目录的 basename，与深度无关**。实测 7 个流行技能仓、**290/290** 个技能全部成立（形态有四种：`skills/{name}`、`skills/{分组}/{name}`、`plugins/{插件}/skills/{name}`、`{分组}/skills/{name}`，仓内深度 2–4 不等）；主流 CLI 的 `getSkillFolderPath()` 用的正是它（砍掉结尾的 `/skill.md`）。**我们的 `skill_md_key()` 只认仓根级或单层 `*/SKILL.md`，是这些实现里唯一更窄的**——接任何外部来源时，这是第一处要改的地方，否则每来一个来源都要为它的目录习惯再打一次补丁。walk 时还须按 basename **精确等于** `SKILL.md` 匹配：用 `endsWith("skill.md")` 会误收 `.changeset/xxx-skill.md` 这类文件（`mattpocock/skills` 上实测 38 与 37 的差就是它）。
- **三家商业目录一条索引都不发**，各有各的堵法（这就是上面「核四条」的由来）：`skills.sh` 是 SPA 兜底 200 + 私定 `/api/search` / `/api/download`（且 `robots.txt` 明确 `Disallow: /api/`），下载是内联文件内容的 JSON 而非包；`skillhub.tencent.com` / `skillhub.cn` 任意路径都回 SPA HTML，真索引在 `api.skillhub.tencent.com`，且**任何地方都不给包的 `sha256`**、下载地址是明文 http 且跨源、靠 302 跳 CDN、包是根级 `SKILL.md`（四条同时踩中，见上「核四条」）；`clawhub.ai` 三条路径全 404（它是 git 背书的目录，本就没有 HTTP 索引）。要用这类市场，得先写适配器，并修订「不跟随重定向」与「非回环必须 https」两条安全口径——**那是产品决定，不是接一根线**。
- **GitHub 是唯一不必修订上面两条安全口径的来源**（`https` + `https://codeload.github.com/{owner}/{repo}/zip/{ref}` **200 直返、不重定向**），但它有三条实测约束，决定了「改成从 GitHub 下载」要写多少东西：① **下载形态只能用 codeload 直连**——`https://api.github.com/repos/{o}/{r}/tarball/{ref}` 是 **302** 跳 codeload，走它会被「不跟随重定向」当场拒掉；② **摘要必须锚在 commit SHA，不能锚 zip 字节**——GitHub 的 `ETag` **不是**所收字节的 `sha256`（实测 `937f5799…` vs `274a83…`），而且**同一个 commit 换 URL 形态字节就变**（分支名取 237506 B、SHA 取 248450 B）**而 `ETag` 相同**：GitHub 认的是**内容身份**、不认**字节身份**，而本系统（上一条「摘要校验」）是字节身份；③ **仓内 `.claude-plugin/marketplace.json` 可作便捷索引但不能当底座**——7 个流行技能仓里 **6 个**有（`anthropics/claude-code` 那份还带 `$schema`），可它是**插件**索引（技能在插件里要再下钻），且 `vercel-labs/agent-skills` 就没有，所以底座仍是「按上一条判据扫目录」。详见 `.scratch/agentpipeline-github-market/issues/01-github-as-market-source.md`（三处口径的取舍、镜像实测、逐条复现命令）。
- **E2E ⑬ 用回环离线 registry 钉安装通路**（`market-install.spec.ts` + `frontend/e2e/marketRegistry.ts`，回环明文 http 由决策 177③ 放行）——它钉的是「通过市场装一个技能」这条**产品行为**，与来源是谁无关；打真网络的用例一律 **opt-in**（照 `screenshots.spec.ts` 的 `test.skip` + 环境变量先例），不进默认质量门。

> **摘要 ≠ 安全**（本票明确不做的部分）：摘要校验**只能**证明「没被改过」，证明不了「内容是善意的」。签名与人工审核队列不在本批——善意性由票 11 的装前预览与信任标记承担。用户若误以为摘要=安全，就会跳过票 11 的预览直接装，所以这条边界写在这里与 `market.rs` 的模块头（错误提示只报事实与动作，不复述这段定位说明）。

**frontmatter 四键**（决策 172，票 02）——`SKILL.md` 的 frontmatter 由文本剥离升级为**解析**，只用四个键，**不引 YAML 依赖**（逐行 `key: value`，值与键都用既有文本口径）：

| 键 | 语义 | 落点 |
|---|---|---|
| `description` | 一句话说明 | 目录态渲染成 `- {name}: {description}`（渐进披露的载体） |
| `disable-model-invocation` | `true` = 手动触发型，**默认不自动注入** | 不进目录态、`Skill` 工具也不给后门。上游 27 个技能里 **14 个**带此键 |
| `license` | 许可声明（如 `Apache-2.0`） | 只记录不判定——再分发问题由「不内嵌」整体解掉（决策 172①） |
| `allowed-tools` | 上游规范里的工具授予声明 | **只解析不生效**：本系统没有「工具权限授予」这一层，误当权限会把「声明」读成「授权」 |

`name` 若写了，**必须与所在目录同名**，否则启动 fail fast；未知键忽略（不因内容拒绝任何合法 markdown，决策 172②）。

**装前预览与信任标记**（决策 172④⑤，票 11）——摘要校验只能证明「没被改过」，证明不了「内容是善意的」，故市场准入自带可见性。两个端点共用同一个组装函数（口径一处）：

| 端点 | 场景 |
|---|---|
| `GET /skills/{name}/preview` | 已安装技能；技能不在技能根下 → 404 |
| `POST /skills/preview?name=` | **装前**：请求体是 zip 原始字节（与 `POST /skills/import` 同形态），第 ③ 项扫的是**包里的字节** |

响应恒为三项：**① 推荐去向**（阶段 + 理由，来自 `STAGE_RECOMMENDATIONS`）、**② 注入模式与信任态**（每条引用它的阶段级 / 节点级声明的 `mode` / `trusted` / `bare`；未被引用时给出默认形态与说明）、**③ 正文特征扫描**——`run_command` / 网络调用（`curl` / `wget` / `http(s)://` 字面量 / `fetch(` / `reqwest`）/ 密钥路径（`.env` / `.ssh` / `.pem` / `credentials` / `id_rsa` / `id_ed25519` / `api_key`），**逐行列出**（行号 1 起算 + 该行原文）。

> **③ 是告知，不是准入判定。** 没有任何一条路径会因为扫描命中而拒绝安装：正则既拦不住变形（`c""url`、变量拼接、base64）又会误伤合法技能（`rtk` 正文里有 `curl` 字样）。风险由预览 + 信任标记 + 工具层出口控制（票 12）承担。拿不到正文时（装前包里没有 `SKILL.md`，或磁盘上那份读不到）`body_available` 为假，界面显示「无正文可扫」而不是「未发现特征」——后者是虚假的安心。

**显式信任转换**：`PUT /skills/{name}/trust {"trusted": bool}` 把引用该技能的**每一条**声明（阶段级 + 全部节点级）就地改写，再走 `PUT /stage-configs` 那道校验门落盘（一处不过则一条都不写）。撤销信任撞上 `full` 声明时**拒绝**并给出可操作提示（先把该处改成 `name` 再撤销），**不静默降级**——静默改注入模式会悄悄停掉一个正在生效的知识源。技能未被任何配置引用时返回 `changed: 0` 与说明，不报错。

**阶段推荐与一键安装**（决策 172①，票 16）——推荐清单的投递载体是**界面**：清单是代码内常量（`STAGE_RECOMMENDATIONS`，只放 `阶段 → 技能名 + 理由`，**不内嵌任何正文**），经 `GET /skills/recommendations` 下发并附「装没装」与「被谁引用」，未安装的项界面显示「未安装」而不是报错。筛选判据只有一条硬约束：**不带 `disable-model-invocation`**（手动触发型不该当常驻知识推荐）。`POST /skills/install {stage, name, overwrite}` 把「装到技能根 + 写进该阶段配置」合成一步，**强制不绕过票 11**：写入的声明只能是 `name` 模式 + 未信任，响应体带回完整三项预览。失败四类可归因（技能不存在 404 / 来源未放行 400 / 摘要不符 400 + `detail` / 网络失败 502）。**已安装的技能可直接启用**（票 15 的界面约定）：技能已在技能根里且未显式 `overwrite` 时跳过下载、只写配置，响应里的 `note` 说明未重新下载——同名的字节不被悄悄替换，「已装但没在这个阶段启用」也不再是一条走不通的路。

**三态渲染**（决策 172④，票 05）——`## 已启用技能` 段里的每个技能按下表之一呈现，形态由声明里的 `mode` 决定：

| 形态 | 渲染 | 何时用 |
|---|---|---|
| 全文态 | `### {name}` + 正文 | `mode: "full"`（裸字符串的默认解释） |
| 名字态 | `- {name}` | `mode: "name"`——正文由 `Skill` 工具按需拉取，不进 system prompt（PATH 工具型技能退场后，这是名字态的唯一来源） |
| 目录态 | `- {name}: {description}` | 未被声明、仅在可用池的技能（渐进披露的落点） |

**只有全文态的正文进 system prompt**，因此 `prompt_template_hash` **只对全文态敏感**（决策 137）：名字态与目录态换正文不会造成 hash 抖动。目录态**不含正文**，`disable-model-invocation: true` 的技能不进目录（选型 D）。正文「存在且非空」在启动与 `PUT /stage-configs` 时 **fail fast**（与 §10.6.4 的 `persona_path` 同口径）；未安装却被引用的技能名同样 fail fast（决策 185 之后「只看技能根」，PATH 里有没有同名可执行文件都不改变判定）。

**技能声明字段形态**（决策 172④）：`string | {name, mode, trusted}` 的混合数组。裸字符串按 `{mode: "full", trusted: false}` 解释（**向后兼容今天的配置行，零迁移**）；对象形态的 `mode` 缺省 `full`、`trusted` 缺省 `false`。**未信任技能不得以 `full` 模式保存**——写入/启动校验直接拒绝，须显式确认信任或改用 `mode: "name"`。这道信任门**只对显式对象生效**：裸字符串是信任概念出现之前手写的配置行，一视同仁会使既有配置全部失效。

**节点级技能：** 阶段级 `skills_json` 无法区分节点，而同一阶段的不同节点职责可能互斥（architect-design 的 `validate_input` 提问、`execute` 写 `design.md`、`validate_output` 校验）。因此技能也可在 `node_overrides_json[node].skills` 声明——见 §10.6.3。有效集 = `mandatory_skills ∪ 阶段级 skills_json ∪ 节点级 skills`（**只增不减**；同名技能的 `mode` / `trusted` 由**更具体的一层**决定，即节点级覆盖阶段级，位置保持首次出现处以维持声明顺序）。

**`Skill` 工具（决策 172③）**——按名加载技能正文，**作为 tool result 进 `messages`**，不进 system prompt（因此不改 `prompt_template_hash`）。工具名与上游同名是功能性决定：上游技能的正文里写着 `Call the Skill tool with "grilling"`，同名使这些正文**无需改写即可执行**。三条边界：

- **由阶段声明启用**，不在 `MANDATORY_TOOLS` 里；但当一个节点存在名字态 / 目录态技能时自动放行（否则那批技能就是断腿的指针）。
- **未知技能名返回错误文本而非 `Err`**：`Err` 会被算作工具失败并累计 `tool_retry_max`，模型写错一个名字就能打挂整个节点；返回文本让模型自行纠正。
- **读技能根（loader 侧），不经 `FileToolPolicy`**：技能根与 `{home}/data/`（provider 密钥明文存储，决策 112）同父，放宽为 agent 可读等于交出密钥。`disable-model-invocation: true` 的技能不加载（目录不广告它，工具也不给它开后门）。

**兄弟文件一级展开（决策 172③，票 07）**——技能正文里的相对 markdown 引用（`[tests.md](tests.md)`、`[UI.md](UI.md)`）在加载时**内联**，使 `tdd`、`prototype`、`codebase-design` 这类带兄弟文件的技能不再指向死链（文件工具被锁在 worktree + 任务目录内，读不到技能目录）。规则：**只展开一级**（被内联文件里的引用不再展开）；目标必须在该技能目录**之内**（拒绝 `../` 穿越、绝对路径，以及 canonicalize 后逃出目录的符号链接）；**缺失的兄弟文件报错**并指出技能名 + 文件名（全文态在启动校验即 fail fast，`Skill` 工具路径返回错误文本）；**非 `.md` 引用不展开不执行**（本系统无脚本执行语义）。全文态**也**展开——否则全文态下兄弟引用仍是死指针。

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
  skills?: SkillDecl[];            // 加载的 skill（与基线取并集）；形态见下方「技能声明字段形态」（决策 172④）
  mcp_servers?: string[];          // 启用的 MCP（与基线取并集）

  // ── 节点级覆盖 ──
  node_overrides?: {
    [node: string]: Partial<StageAgentConfig> & {
      idle_timeout_sec?: number;   // 覆盖全局 node_idle_timeout_sec（决策 66）
      max_duration_sec?: number;   // 覆盖全局 node_max_duration_sec
      skills?: SkillDecl[];        // 该节点专属技能（与阶段级取并集，决策 170；形态同上）
      resume_continuation?: boolean; // 覆盖阶段级续接开关（决策 180）
    };
  };

  // ── 会话续接（决策 180，票 13）──
  resume_continuation?: boolean;   // pending → resume 时续接上一 attempt 的对话；默认 false
}

// 技能声明：裸字符串 = { name, mode: "full", trusted: false }（旧配置行零迁移）
type SkillDecl = string | { name: string; mode: "full" | "name"; trusted?: boolean };
```

> **节点级 `skills` 的存在理由（决策 170）：** 阶段级 `skills` 是整阶段生效的，而同一阶段的节点职责可能互斥。典型用例——architect-design 的 `validate_input` 需要「拷问」（`grilling`：把设计树走到没有悬空分支、只把决定问用户），`execute` 需要「综合成规格」（`to-spec`：不再提问、把已定内容写成 `design.md`）。若只在阶段级声明，写文件的节点也会拿到「不断向用户提问」的指引，二者只能互相打架。

#### 10.6.4 合并与校验规则

| 配置项 | 合并方式 | 校验 |
|---|---|---|
| provider | 阶段引用 `provider_id`（无则用系统默认） | 该行的厂商必须 ∈ `supported_adapters` **且** 该行 `enabled = 1`，否则**加载时拒绝**（决策 103 / 111） |
| provider（运行时解析） | `node_overrides > task.model_override > 阶段 provider > 全局默认`（决策 129） | `model_override` 引用的 provider_id 在设置时（`POST /tasks/{id}/model-override`）须过同一校验 |
| persona | 阶段 prompt + 基线强制前言 | 必须存在且非空 |
| tools | `基线 mandatory_tools ∪ 阶段 tools − forbidden_tools`；节点存在名字态 / 目录态技能时自动并入 `Skill`（决策 172③，否则那批技能是断腿的指针）。扩展工具 `spawn_sub_agent` 只由阶段声明启用，**且其子代理的工具集固定只读、不继承此处并集**（票 08 的安全边界） | 不能移除 mandatory_tools |
| skills | `基线 mandatory_skills ∪ 阶段 skills ∪ 节点级 skills`（决策 170，只增不减）；同名技能的 `mode` / `trusted` 由更具体的一层决定（节点级覆盖阶段级） | 不能移除 mandatory_skills；引用的 skill 名字必须存在，否则 fail fast；知识型技能的**正文必须存在且非空**，否则 fail fast（与 `persona_path` 同口径）；**未受信任的技能不得以 `full` 保存**（决策 172④）——写入与启动两侧都拒绝，须显式确认信任或改用 `name`。这道门**只对显式对象生效**：裸字符串是信任概念出现之前手写的配置行，一视同仁会使既有配置全部失效（零迁移） |
| resume_continuation | 节点级 > 阶段级 > **关**（决策 180；照 `idle_timeout_sec` 的分层） | 无——它是一个开关，不改任何既有校验 |

**工具层出口策略（决策 179，票 12）**——`run_command` 的网络出口按 allowlist 放行，**默认只放行回环**（`[pipeline] egress_allow_hosts` / `egress_allow_all`，见 §10.6.5）。它受 `Settings` 控制并经 `ToolExecutor` 的执行点强制，与 §10.6.2 的 `file_tool_policy` 是同构的两件事：**都只约束工具层，都不是系统级沙箱**。被拒的调用落 `kanban_node_commands`（与放行的命令同表）并返回可归因的 `PolicyDenied` 报文。残余风险与 OS 级沙箱候选见 `docs/operations.md` §12.15。

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

# run_command 的出口放行主机（决策 179，票 12）。**默认空 = 只放行回环**
# （localhost / 127.* / ::1）。条目形态：精确主机、`*.example.com`（子域通配，落在点边界上）、
# `*`。写错在解析期 fail fast。这是「市场下载的技能 + agent 有无限 shell」在工具层的兜底：
# 只约束 agent 主动经 run_command 发起的调用，**不是安全边界**（残余风险见 §12.15）。
# egress_allow_hosts = ["api.example.com", "*.internal.example.com"]
# 显式放行全部出口。默认 false——未配置时不得静默变成「全部放行」。
# egress_allow_all = false

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

[skills]
dir = "~/.agentpipeline/skills"      # 覆盖技能根；缺省回落 {home}/skills（决策 172）
# 可直接指到已有的技能生态目录，如 ~/.zcode/skills——该目录下的 {name}/SKILL.md
# 被发现、校验并可用；未配置时行为与之前逐字相同。
# 覆盖目录由用户自己维护：本系统不会创建它，也不会改它的权限。
# 技能文件 frontmatter 里若写了 name，必须与所在目录同名，否则启动 fail fast。

[market]
# 技能市场的来源白名单（决策 172⑤，票 10）。**默认空 = 不允许任何远程安装**。
# 判定按 origin（scheme://host[:port]），不接受带路径的写法；列表第一个来源同时用作
# 索引地址（{source}/index.json）。未放行的来源在搜索与安装两侧都被拒。
# allowed_sources = ["https://skills.example.com"]
```

> **配置校验姿态（票 16 / 决策 172）：** `config.toml` 中未知的 section / 键一律**拒绝启动**
> （`deny_unknown_fields` 施加于 `Config` / `ServerConfig` / `PipelineOverrides` /
> `LoggingConfig` / `PromptsConfig` / `SkillsConfig` / `MarketConfig`），不静默忽略——与决策
> 47 / 103 / 134 的 fail fast 姿态一致。`[logging]` 的 `format` 与已废弃 `json_file` 同时出现
> 同样报错。
>
> **升级注意（行为变化）：** 此前拼错或多余的键会被静默忽略、按默认值运行；现在**启动即报错**。
> 这是有意的收紧——静默忽略会让「配置写了却没生效」无从察觉。

**阶段级 Agent 配置**存储在 SQLite 数据库中，通过前端界面配置。每个阶段可独立设置 provider（引用 `providers` 表的 `provider_id`）、tools、skills、超时覆盖。系统最小基线（mandatory_tools、mandatory_skills、`file_tool_policy`）在代码中硬编码，不可覆盖。模型上下文窗口随 `providers` 表的一行存在一起（决策 46 / 111）——**阶段不单独存 model**，换模型即换 `provider_id`，这样 L0 容量预估（§12.13.3）查找窗口大小的路径唯一。伪阶段（`project_analysis` / `conflict_check` / `validator_cross_check`）复用同一配置机制（决策 67 / 87 / 134）；`cross_family_judge = true` 时 `validator_cross_check` 必须已配置 provider，否则配置加载 fail fast。

**节点级技能配置示例（决策 170 / 172）——给 architect-design 配「拷问 + 综合成规格」：** 技能来自用户目录（内嵌技能已退场，决策 172①），故先用设置页导入或手工放置 `~/.agentpipeline/skills/{name}/SKILL.md`，再在 `node_overrides_json` 里按节点声明（`PUT /stage-configs/architect-design` 整条替换该阶段配置）：

```json
{
  "validate_input": { "skills": ["grilling"] },
  "execute":        { "skills": [{ "name": "to-spec", "mode": "name", "trusted": true }] }
}
```

`validate_input` 因此拿到「把设计树走到没有悬空分支、只把**决定**问用户（事实自己查）、经 `submit_metadata.blockers` 提问」的指引；`execute` 声明为**名字态**，正文不进 system prompt，由 agent 需要时调 `Skill` 工具按需拉取（决策 172③）。也可以把 `[skills] dir` 指到已有生态目录（如 `~/.zcode/skills`）整体换掉技能根（决策 172）。技能 `SKILL.md` 的 frontmatter 里若写了 `name`，必须与所在目录同名；引用的兄弟文件（`[tests.md](tests.md)`）必须存在且在该技能目录内——两条都在启动与 `PUT /stage-configs` 时 fail fast。回滚：`DELETE /stage-configs/architect-design` 撤销该阶段覆盖。

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
