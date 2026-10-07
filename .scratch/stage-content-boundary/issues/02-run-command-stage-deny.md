# 02 run_command 按阶段双层禁用：review / test-design

Status: ready-for-agent
决策号预留: 396（落地时续写 docs/decisions.md，扩展决策 206 的档位体系、
复用决策 179 egress 的拒绝形状）

## 目标

grilling Q5=C + Q8=C：review 与 test-design 的模板全链路零依赖 run_command
（`templates.rs:173-218, 259-287`，探查已核实），对这两个阶段双层禁用——
工具定义层不给（去诱惑），执行层兜底（防伪造工具名直调）。

## 改动一：工具定义层过滤

`crates/core/src/pipeline/model_request.rs:755-841` 的 `tool_defs()` 增加
按节点过滤：`AgentNodeKind` 属于 `Stage::Review` 或 `Stage::TestDesign` 的
agent 请求，`tool_defs` 不含 `run_command`。

- 过滤按**节点**（`AgentNodeKind::of(stage, node)`，`model_invoke.rs:1737-1756`）
  而非仅 stage——test-design 的 validate_input 节点同样不给。
- `MANDATORY_TOOLS`（`client.rs:214-240`）语义复核：若 `run_command` 在
  mandatory 集合里，需把「mandatory」改为「mandatory unless stage-denied」，
  未知工具名报错逻辑（`model_request.rs:820-827`）不动。

## 改动二：执行层兜底

所有 agent 命令的唯一执行点 `ToolExecutor::run_command`
（`crates/core/src/agent/tools.rs:2407`）。在白名单闸之后、egress 检查
（`tools.rs:2429`）之前插入：

```rust
if matches!(ctx.stage, Stage::Review | Stage::TestDesign) {
    // 拒绝也落台账，形状照 egress denied（tools.rs:2422-2444 的
    // record_command_start + 专用退出码）
    return Err(Error::PolicyDenied(
        "run_command 在本阶段被禁用（本阶段工具面：read_file / write_file / submit_metadata）",
    ));
}
```

- `ctx.stage` 已在 `ToolCallContext`（`tools.rs:381`），拿改写前原命令的
  顺序不变量（`tools.rs:2426-2428`）不受影响。
- 拒绝落台账：照 egress 的形状新增专用记号（或复用 `EGRESS_DENIED_EXIT_CODE`
  的落库路径另立 kind），值班长 / run 审计可查。
- 闸门命令与系统命令不经此路（`run_system_command` 直跑，
  `executor.rs:1745`），不受影响。

## 改动三：模板零告知（有意为之）

review / test-design 模板**不改**——它们本来就只叙述三个动作，加
「run_command 被禁」反而引入 agent 不需要知道的概念。执行层拒绝信息
已带工具面清单，回灌自纠足够。

## 断言清单

单测：

1. `tool_defs()` 对 Review/TestDesign 的四个节点（review.execute、
   test-design.validate_input/execute/validate_output）均不含 `run_command`；
   其余阶段不变（快照式逐节点断言，防过滤面扩大）。
2. 执行层：构造 `ctx.stage = Review` 直调 `run_command` → `PolicyDenied`，
   台账有拒绝记录；`ctx.stage = Develop` 不受影响。
3. 双层一致性测试：对每个 `(stage, node)` 断言「def 层给的工具 ⊇ 执行层放行的
   工具」——防未来单改一层造成 agent 可见但必拒的工具（浪费 attempts）。

e2e：

4. mock review agent 伪造调用 run_command → 断言 `PolicyDenied`、任务推进
   不受影响、台账可见。
5. mock test-design agent 同上。

## 留观项（不在本票）

architect-design / develop-design 的模板 run_command 依赖未核实。实施本票时
顺带 grep 两模板全文：若确认零依赖，开独立裁决（同样双层禁用，机制已就绪，
纯配置）；若有依赖（如读项目结构），记录依赖点后关账。
