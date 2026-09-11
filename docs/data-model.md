# 数据模型与 Pending/Resume 映射

> 拆分自 agent-pipeline.md（原 §4–§5）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

> submit_metadata 各字段的权威定义在 §4.2；[pipeline-spec.md](pipeline-spec.md) §6 的 validate 规则与 [agents.md](agents.md) §10.3 的 prompt 模板按字段名引用，不在此重复定义。

## 4. 数据模型

### 4.1 任务主结构

```typescript
interface Task {
  id: string;
  project_id: string;          // 所属项目
  title: string;
  description: string;
  status: TaskStatus;
  cursors: NodeCursor[];       // 全部活跃游标；串行阶段恒为 1 个，并行阶段 2 个（决策 80）
  current_stage: string;       // 焦点游标（看板展示 / 筛选 / 兼容旧接口），不承载执行状态
  current_node: string;        // 焦点游标的节点
  pending_reason?: PendingReason;   // 任务级 pending：等任一被阻塞游标解除（决策 82）

  // ── 隔离 ──
  worktree_path: string;       // git worktree 工作目录
  branch_name: string;         // 隔离分支名

  // ── 成本 ──
  total_tokens: number;        // 累计 token（= 该任务所有 kanban_node_runs 行求和，决策 100）
  total_calls: number;         // 累计 LLM 调用次数 = 调 LLM 的 run 行数（main + 子代理 + 伪阶段，不含 agent_type="system"；决策 130）

  // ── 依赖 ──
  depends_on: string[];        // 前置任务 ID 列表
  blocks: string[];            // 依赖本任务的任务（由 kanban_task_deps 派生，不落库）

  // ── review 模式 ──
  review_mode: "agent" | "human";  // agent 自动评审 / 人工评审

  // ── 模型覆盖（决策 105） ──
  model_override?: string;         // 任务级 provider_id 覆盖，只影响本任务后续节点

  // ── 归档与提醒 ──
  archived_at?: string;        // 归档时间戳（软删除），非 TaskStatus 枚举值
  stalled: boolean;            // pending 超过 pending_timeout_hours 的超时标志

  created_at: string;
  updated_at: string;
}

type TaskStatus =
  | "queued"       // 排队等并发准入（决策 98）。与 waiting 正交：queued 等的是并发名额
  | "pending"      // 等待用户操作
  | "waiting"      // 等待依赖任务完成
  | "running"      // 正在执行
  | "done"         // 终态：成功
  | "failed"       // 终态：失败。唯一入口是用户在 pending 卡片选择"终止任务"（决策 70），重试后回到 init
  | "cancelled";   // 终态：用户取消

/**
 * 活跃游标（决策 80）。执行状态的唯一事实来源，落库为 kanban_node_cursors 表。
 * kanban_tasks.current_stage / current_node 只是它的一个投影，供看板展示与筛选。
 *
 * 生命周期（决策 90 / 91 / 113）：
 *   - 创建：POST /tasks 与 Task 记录同事务插入单条 main 游标（stage=init, node=execute）。
 *           waiting / queued 任务同样有游标，因此 dependency_failed 这类 pending 有处可挂。
 *   - 分裂：architect-design 通过后，main 行就地改写为 branch="develop-design"（cursor_id 不变），
 *          并插入 branch="test-design" 行（新 cursor_id，ULID）。partial UNIQUE(task_id, branch) 保证分裂幂等。
 *   - 合并：sync-check 通过后，一个事务内把两条分支行置 status="archived"、插入单条 main 行指向 develop.execute。
 *   - 回退：backtrack 同事务内归档两条分支行、插入单条 main 行指向 architect-design.validate_input。
 *   - 终态：游标行保留（审计与 checkpoint 回放用）；"重试"时归档全部旧行、插入单条 main 游标指向 init.execute。
 *   - 游标行**永不物理删除**（决策 113）：kanban_node_runs.cursor_id 的 NOT NULL 外键因此永不悬空；
 *     归档行不受 partial UNIQUE 约束，同一分支之后可插入新行（新行新 cursor_id）。
 *   - 注意：并行区间不存在**活跃的** main 游标（已被改写为 develop-design）。
 */
interface NodeCursor {
  cursor_id: string;           // 游标标识
  branch: string;              // "main" | "develop-design" | "test-design"（并行分支消歧）
  stage: string;
  node: string;                // "validate_input" | "execute" | "validate_output"
  status: "active"             // 可继续执行
        | "waiting_join"       // 已到达 join 边界，等同阶段其余分支（决策 82）
        | "pending"            // 该游标自身被阻塞
        | "archived";          // 被合并/回退/重试取代的历史行，只读保留（决策 113），不参与执行
  validate_attempts: number;   // 本游标内 validate_output → execute 循环计数（跨阶段跳转时重置为 0，决策 43）
  skipped_to_join: boolean;    // 用户对本分支执行 skip 后被置位（决策 93）：表示该分支"放行到达边界"。
                               // sync-check.execute 读到该标志时把对应分支视作 readiness=true。
                               // 这不修改 kanban_stage_outputs 的产出元数据——不伪造 agent 的结论。
  pending_reason?: PendingReason;  // 游标级 pending 原因（status = "pending" 时存在）
}
```

### 4.2 阶段输入输出

```typescript
interface StageIO {
  task_id: string;

  // ── init ──
  init_output: {
    task_title: string;
    task_description: string;
  };

  // ── architect-design ──
  design_doc: {
    design_doc_path: string;         // 设计文档文件路径（Markdown）
    affected_files: string[];        // 涉及的源码文件路径列表
    new_symbols: NewSymbol[];        // 本次新增的公开符号（冲突检测第一层依据）
    conflict_warnings: ConflictWarning[];
  };

  // ── develop-design ──
  dev_doc: {
    dev_doc_path: string;            // 开发方案文件路径（Markdown）
    file_changes: FileChangeSpec[];  // 预期文件变更列表
    readiness: boolean;
    blockers?: string[];
  };

  // ── test-design ──
  test_design: {
    test_scenarios_path: string;     // 测试场景文档文件路径（Markdown，非代码）
    test_scenarios: TestScenario[];  // 测试场景清单
    readiness: boolean;
    blockers?: string[];
  };

  // ── sync-check ──
  sync_decision: {
    decision: "proceed" | "backtrack";
    dev_readiness: boolean;
    test_readiness: boolean;
    dev_blockers?: string[];
    test_blockers?: string[];
  };

  // ── develop ──
  code_changes: {
    branch_name: string;
    changed_files: FileChangeSpec[];
    unit_test_files: FileChangeSpec[];
  };

  // ── review ──
  review_result: {
    approved: boolean;
    review_report_path: string;
    required_changes?: FileChangeSpec[];
  };

  // ── test ──
  test_result: {
    passed: boolean;
    test_report_path: string;       // 测试报告（含执行结果）
    failures?: TestFailure[];       // 失败用例及根因分类（由 test.execute 的 agent 给出）
    gate_recheck?: boolean;         // 本次 test.execute 是被 merge 测试闸门打回后的复检（决策 85），
                                    // prompt 需追加闸门失败输出上下文
  };

  // ── merge ──
  merge_result: {
    diff_path: string;              // diff 文件路径（unified diff 格式）
    diff_stats: DiffStats;          // 变更统计（文件数、增删行数）
    base_commit: string;            // 生成 proposal 时的 `{base_ref}` commit SHA（决策 96）：
                                    // 阶段 B 入口比对该值；不一致说明基准已前移、diff 已过期，须重走阶段 A
    gate: "pass" | "fail";          // 合入前测试闸门结果（单元 + 集成）。**与 approval 正交**（决策 95）：
                                    // 闸门失败是执行结果，不是审批状态，不得塞进 approval 枚举
    gate_failures: number;          // 闸门失败累计次数（决策 108）。存 kanban_stage_outputs.metadata_json，
                                    // 跨阶段跳转不重置（否则 merge ↔ test 循环不终止），
                                    // 超 validate_retry_max 才 pending(retry_exhausted)
    gate_failure_output?: string;   // 闸门失败输出摘要，作为 test.execute 复检的输入（决策 85）
    conflict_files?: string[];      // 冲突文件列表（backtrack 时传入 develop）
    approval: "none"                // 尚未生成 proposal
            | "pending"             // proposal 已生成，等待用户审批
            | "approved"            // 用户已批准，resume 后执行合入（决策 72）
            | "returned";           // 用户选择返回修改，回 develop.execute
    status: "pending_approval"      // 等待用户审批（合入判定改看 approval，本字段表示结果）
          | "merged";               // 已合入（用户批准后由 merge.execute 执行合并）
  };

  interface DiffStats {
    files_changed: number;
    insertions: number;
    deletions: number;
    file_details: FileDiffDetail[];
  }

  interface FileDiffDetail {
    path: string;
    additions: number;
    deletions: number;
    status: "added" | "modified" | "deleted";
  }

  // ── done ──
  done_result: {
    success: boolean;
    summary: string;
  };
}

interface TestScenario {
  id: string;
  name: string;                    // 场景名称，如 "用户登录成功"
  description: string;             // 场景描述
  preconditions: string[];         // 前置条件
  steps: string[];                 // 测试步骤
  expected_result: string;         // 预期结果
  priority: "high" | "medium" | "low";
}

interface FileChangeSpec {
  path: string;
  action: "create" | "modify" | "delete";
  content_hash?: string;
}

interface NewSymbol {
  name: string;                    // 符号名，如 "validate_email"
  kind: "function" | "struct" | "enum" | "trait" | "method" | "const" | "module"
      | "class" | "interface" | "type";   // 后三者覆盖非 Rust 目标项目（决策 120）
  module_path: string;             // 模块/包路径，如 "crate::auth::validator"
  file_path: string;               // 所在文件
}

interface TestFailure {
  test_name: string;
  error_message: string;
  failure_cause: "test_issue" | "code_issue";  // 用例问题 / 业务代码问题，由 execute 阶段判定
}

interface ConflictWarning {
  task_id: string;
  task_title: string;
  overlapping_files: string[];
  overlapping_symbols?: string[];  // 符号名交集（第一层检测）
  duplicate_risk?: "low" | "medium" | "high";  // 语义比对结论（第二层检测）
}

interface PendingReason {
  type:
    | "info_insufficient"
    | "conflict_wait"
    | "retry_exhausted"
    | "user_decision"
    | "merge_approval"      // 等待用户在 GUI 审核 diff 并决定是否合入
    | "human_review"        // 等待人工评审（review_mode = "human"）
    | "dependency_failed"   // 依赖任务失败
    | "context_overflow"    // 上下文超限，压缩后仍无法容纳
    | "timeout";
  stage: string;
  node: string;
  message: string;
  suggested_actions?: string[];
  /**
   * 结构化上下文。约定字段（决策 92 / 102）：
   *   kind: "duplicate_risk" | "dirty_worktree" | "test_code_issue" | ...  // 用于 (type, kind) 查动作表
   *   conflict_task_ids: string[]  // conflict_wait 专用：**全部**冲突任务 id（不是单个）。
   *                                // 全部终态且重跑第一层比对后仍无交集才自动恢复（决策 102）
   *   gate_failure_output?: string // 闸门失败详情，传给 test.execute 复检
   */
  context?: Record<string, unknown>;
}
```

> **注意：`backtrack` 不是 pending。** sync-check 判定 backtrack 是**自动流转**（决策 83），不进入 pending 状态，因此不在 `PendingReason.type` 枚举内，也不在 §5 映射表内——它属于 §7 的并行流转规则。

**文件目录约定：**

```
~/.agentpipeline/tasks/{task_id}/
├── design.md              # architect-design 产出
├── dev-plan.md            # develop-design 产出
├── test-scenarios.md      # test-design 产出（业务测试场景文档）
├── review-report.md       # review 产出
├── review-diff.diff       # review 产出（系统生成的变更 diff，仅 review_mode=human，决策 124）
├── backtrack-feedback.md  # sync-check backtrack 时写入的双方 blockers（决策 126）
├── test-report.md         # test 产出（执行结果）
└── merge-proposal.diff    # merge 产出（diff 文件）

注：测试代码在 worktree 内 tests/ 目录（*_test.rs）
```

---

## 5. Pending → Resume 映射

| 触发阶段 / 节点 | pending_reason.type | 恢复后的入口 | 用户看到的信息 |
|---|---|---|---|
| architect-design / validate_input | info_insufficient | continue → validate_input（用户补充后） | 缺少哪些信息，输入框让用户补充 |
| architect-design / execute | conflict_wait | 自动恢复（冲突任务终态后）→ execute | 冲突任务、重叠文件/符号 |
| architect-design / execute | user_decision（`context.kind=duplicate_risk`） | goto develop / 取消其一（"合并任务"已移除——决策 132，v1 无端点，用户自行取消一方后重建） | 两份设计并排对比 + 语义重复风险 |
| architect-design / validate_output | retry_exhausted | 用户选择：goto execute 或 skip（强制进入下一阶段） | 已产出的设计文档 + 不足之处 |
| develop-design / validate_input | user_decision | 用户选择：goto architect-design 或 skip | 设计文档不足以支撑开发的原因 |
| develop-design / validate_output | retry_exhausted | 用户选择：goto execute 或 skip | 已产出的开发文档 |
| test-design / validate_input | user_decision | 用户选择：goto architect-design 或 skip | 设计文档不足以支撑测试设计的原因 |
| test-design / validate_output | retry_exhausted | 用户选择：goto execute 或 skip | 已产出的测试场景文档 |
| develop / validate_output | retry_exhausted | 用户选择：goto execute 或 skip | 测试失败详情 |
| review / validate_output | user_decision | 用户选择：goto develop.execute（修复）或 skip（强制通过进入 test） | 评审意见 |
| review / validate_output | human_review | 用户提交评审结果（`POST /tasks/{id}/review`）：通过 → test，不通过 → develop.execute | 变更 diff + agent 预审报告 + 单元测试结果（review 在 test 之前，此时无集成测试结果） |
| test / validate_output | retry_exhausted | 用户选择：goto execute 或 skip | 失败详情 |
| test / validate_output | user_decision | 用户选择：goto execute（修复用例）或 goto develop.execute（改业务代码） | 失败详情 + 根因分类 |
| test / validate_output | user_decision（来自 merge 闸门失败，决策 85） | 用户选择：goto test.execute（修用例）或 goto develop.execute（改业务代码） | 闸门失败详情 + 根因分类 |
| merge / execute | retry_exhausted | 用户选择：重试 merge.execute / 终止任务（**无 skip**，决策 86） | 冲突文件或闸门失败详情 + 已尝试次数（`gate_failures`） |
| merge / execute | merge_approval | 用户在 GUI 审核 diff 后点击"合入"或"返回修改"（`POST /tasks/{id}/merge/decision`，决策 119） | diff 文件 + 变更统计 |
| merge / execute | user_decision（脏工作区） | 用户选择：我已处理，继续合入 / 取消任务（决策 132："放弃合入"移除，无端点） | 目标分支未提交改动清单 |
| 任何节点 | timeout | 自动按 `agent_retry_max` 重试；耗尽后用户选择：goto execute 或 skip（**merge 除外**：动作集同决策 86，重试 / 终止任务，无 skip——决策 122） | 超时的节点和已耗时 |
| 任何节点 | context_overflow | 用户选择：拆分任务 / 更换长上下文模型 / 取消 | 峰值 token、压缩次数 |
| init（未启动） | dependency_failed | "继续执行"= 忽略失败依赖置回 queued（决策 116）/ 取消任务 / 等待依赖重试（仅依赖 failed 时提供） | 失败的依赖任务清单 |

> **`dependency_failed` 挂在哪条游标：** 任务在 `POST /tasks` 时就会创建单条 main 游标（决策 90），因此处于 `waiting` / `queued` 的任务也有游标可挂——该 pending 挂在 main 游标上（stage=init, node=execute），不是任务级特例。

> **`backtrack` 不在此表：** sync-check 的 backtrack 是自动流转（决策 83），不经过 pending。它不在 `PendingReason.type` 枚举内，属于 §7 的并行流转规则。

> **动作语义（决策 35 / 69）：** `continue` = 清除 pending 后从当前节点继续；`skip` = 清除 pending 并强制流转到下一阶段（落点规则见 §11.3 的 skip 表，决策 93）；`goto` = 清除 pending 并把游标的 `stage` / `node` 置为指定目标。这三者是**恢复动作**，可出现在 `ResumeRequest` 里。除此之外的选项（取消任务、拆分任务、更换长上下文模型——决策 132 已把无端点的「放弃合入」「合并任务」移出动作集）是**旁路动作**：走各自的专用 API（如 `POST /tasks/{id}/cancel`），在 `allowed_actions` 中以 `kind: "resume" | "side_effect"` 区分。可用动作集由后端下发（决策 49），key 为 `(pending_reason.type, context.kind)` —— 因为 `user_decision` 一个 type 下挂了 review 打回、测试 code_issue、脏工作区、语义重复等多套不同动作。**每个 side_effect 动作必须有一个配对的端点**（决策 101 / 119），见 §11.7。

**allowed_actions 权威总表（决策 130）：** 动作集由后端按 `(pending_reason.type, context.kind)` 下发，前端纯渲染；本表是唯一权威定义，新增动作必须同时更新本表与配对端点（决策 101）。`resume` 类走 `POST /tasks/{id}/resume`，`side_effect` 类走配对端点。

| type | context.kind | 动作（kind） | 备注 |
|---|---|---|---|
| info_insufficient | — | `continue`（resume，`requires_input`） | 唯一带自由输入的动作（决策 79） |
| conflict_wait | — | 无用户动作（自动恢复）＋ `cancel`（side_effect） | 冲突任务全部终态且复检无交集后自动 resume（决策 102） |
| user_decision | duplicate_risk | `goto develop`（resume）、`cancel 其一`（side_effect） | 决策 132：「合并任务」移出动作集，合并由用户自行取消一方后重建（自动合并留 v2） |
| user_decision | develop/test-design 输入不足 | `goto architect-design`、`skip`（均 resume） | 决策 94 |
| retry_exhausted | — | `goto execute`、`skip`（resume）＋ `cancel 终止任务`（side_effect） | merge 例外：无 skip，仅 `goto`（重试）/ `cancel`（决策 86） |
| user_decision | review 不通过 | `goto develop.execute`、`skip`（均 resume） | review.validate_output 纯代码判定 approved |
| human_review | — | `approve` / `reject`（side_effect → `POST /tasks/{id}/review`） | 通过 → test；打回 → develop.execute（决策 2） |
| user_decision | test code_issue / 闸门复检 | `goto test.execute`、`goto develop.execute`（均 resume） | 根因分类由 agent 给出（决策 62 / 85） |
| merge_approval | — | `approve` / `return`（side_effect → `POST /tasks/{id}/merge/decision`） | 决策 119 |
| user_decision | dirty_worktree | `continue`（"我已处理，继续合入"）、`cancel 取消任务`（side_effect） | 决策 132：「放弃合入」移出动作集（无端点，与前端评审决议④对齐） |
| timeout | — | `goto execute`、`skip`（均 resume） | merge 例外同 retry_exhausted（决策 122） |
| context_overflow | — | `split_task` / `model_override` / `cancel`（均 side_effect） | 决策 105 |
| dependency_failed | 依赖 failed | `continue`（= 忽略失败依赖置回 queued，决策 116）、`cancel`（side_effect）、`等待依赖重试`（纯等待，无系统变更） | 依赖 cancelled 时无"等待依赖重试"（决策 116） |
