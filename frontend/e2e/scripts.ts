/**
 * Node 侧脚本 DSL（票 18）：与 testkit 的 `Script` 语义对齐——只替换 LLM 响应流。
 *
 * **按「轮」投喂**：每个节点是 `Round[]`，一轮 = 该节点一次运行内按序回放的
 * 工具/元数据步骤。mock 用请求体 `messages.length <= 2`（system + user，见
 * OpenAI 适配器）判定「新一轮节点运行」，因此 pending → resume 后重入同一节点
 * 会消费下一轮——等价于 testkit 注记里的「多轮行为用 set_script 分轮投喂」。
 */

export type Step =
  | { kind: 'tool'; name: string; args: unknown }
  | { kind: 'submit'; value: unknown }
  | { kind: 'text'; text: string };

export type NodeScript = Record<string, Step[][]>;

export const tool = (name: string, args: unknown): Step => ({ kind: 'tool', name, args });
export const submit = (value: unknown): Step => ({ kind: 'submit', value });
export const text = (value: string): Step => ({ kind: 'text', text: value });

export const writeFile = (path: string, content: string): Step =>
  tool('write_file', { path, content });
export const runCommand = (command: string): Step => tool('run_command', { command });

/** 元数据结构（与 crates/core/src/types.rs 的 submit_metadata 逐字段对齐）。 */
export const ValidateInput = (readiness: boolean, blockers: string[] = []) => ({
  readiness,
  blockers,
});
export const ValidateOutput = (passed: boolean) => ({ passed, blockers: [] });

export const ArchitectExecute = (opts: {
  readiness?: boolean;
  affectedFiles?: string[];
  acceptanceCriteria?: Array<{ id: string; description: string }>;
  designDocPath?: string;
}) => ({
  readiness: opts.readiness ?? true,
  blockers: [],
  affected_files: opts.affectedFiles ?? [],
  new_symbols: [],
  conflict_warnings: [],
  acceptance_criteria: opts.acceptanceCriteria ?? [],
  design_doc_path: opts.designDocPath,
});

export const DevelopDesign = (readiness = true) => ({
  readiness,
  blockers: [],
  file_changes: [],
  dev_doc_path: 'dev-plan.md',
});

export const TestDesign = (readiness = true) => ({
  readiness,
  blockers: [],
  test_scenarios: [
    {
      id: 'S-1',
      name: '登录成功',
      description: '登录',
      preconditions: [],
      steps: [],
      expected_result: '成功',
      priority: 'high',
      design_refs: ['AC-1'],
    },
  ],
  test_scenarios_path: 'test-scenarios.md',
});

export const CodeChanges = (taskId: string) => ({
  branch_name: `kanban/${taskId}`,
  changed_files: [],
  unit_test_files: [],
});

export const ReviewResult = (approved = true) => ({
  approved,
  review_report_path: 'review-report.md',
  required_changes: [],
});

export const TestResult = (passed = true) => ({
  passed,
  test_report_path: 'test-report.md',
  failures: [],
  gate_recheck: false,
});

/* ─────────────────────────────── 节点键 ─────────────────────────────── */

export const NODE = {
  archVI: 'architect-design.validate_input',
  archEx: 'architect-design.execute',
  archVO: 'architect-design.validate_output',
  devDesignVI: 'develop-design.validate_input',
  devDesignEx: 'develop-design.execute',
  devDesignVO: 'develop-design.validate_output',
  testDesignVI: 'test-design.validate_input',
  testDesignEx: 'test-design.execute',
  testDesignVO: 'test-design.validate_output',
  developEx: 'develop.execute',
  reviewEx: 'review.execute',
  testEx: 'test.execute',
} as const;

/** 设计三阶段全部通过（design.md 带 AC-1，供 design_refs / sync-check 引用）。 */
export function designRounds(): NodeScript {
  return {
    [NODE.archVI]: [[submit(ValidateInput(true))]],
    [NODE.archEx]: [
      [
        writeFile('design.md', '# 设计\n## 验收标准\n- AC-1 能登录\n'),
        submit(
          ArchitectExecute({
            affectedFiles: ['src/lib.rs'],
            acceptanceCriteria: [{ id: 'AC-1', description: '能登录' }],
            designDocPath: 'design.md',
          }),
        ),
      ],
    ],
    [NODE.archVO]: [[submit(ValidateOutput(true))]],
    [NODE.devDesignVI]: [[submit(ValidateInput(true))]],
    [NODE.devDesignEx]: [[writeFile('dev-plan.md', '# 开发计划\n'), submit(DevelopDesign())]],
    [NODE.devDesignVO]: [[submit(ValidateOutput(true))]],
    [NODE.testDesignVI]: [[submit(ValidateInput(true))]],
    [NODE.testDesignEx]: [
      [writeFile('test-scenarios.md', '# 测试场景\n'), submit(TestDesign())],
    ],
    [NODE.testDesignVO]: [[submit(ValidateOutput(true))]],
  };
}

/**
 * 实现三阶段（develop / review / test）：真写代码 + 真 git 提交。
 *
 * 写的是 **Node 工程**的产物（主流程票 02 的 fixture 是 `npm test` 项目）：
 * `src/lib.js` 由「未实现占位」替换为真实实现，使闸门 `npm test --silent` 真的转绿。
 * 闸门失败路径由 {@link failingGateRounds} 提供——两者共用同一 fixture。
 */
export function implementationRounds(taskId: string): NodeScript {
  return {
    [NODE.developEx]: [
      [
        writeFile(
          'src/lib.js',
          'function add(a, b) { return a + b; }\nmodule.exports = { add };\n',
        ),
        writeFile(
          'tests/acceptance.js',
          "const { add } = require('../src/lib.js');\n" +
            "if (add(1, 2) !== 3) throw new Error('acceptance: add(1,2) !== 3');\n",
        ),
        runCommand(
          `git add -A && git -c user.name=e2e -c user.email=e2e@localhost commit -m 'feat: task ${taskId}'`,
        ),
        submit(CodeChanges(taskId)),
      ],
    ],
    [NODE.reviewEx]: [
      [
        writeFile('review-report.md', '# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n'),
        submit(ReviewResult(true)),
      ],
    ],
    [NODE.testEx]: [
      [writeFile('test-report.md', '# 测试报告\n全部通过\n'), submit(TestResult(true))],
    ],
  };
}

/**
 * 闸门先失败、修好后通过的脚本（主流程票 02 的「闸门失败」用例）。
 *
 * 第 1 轮 develop.execute 写的实现**故意让闸门失败**（`add` 返回错值）并提交——
 * develop 的纯代码 `validate_output` 真跑 `npm test --silent` → 退出码非 0 →
 * lint/test 分流 + `gate_failures` 累加（决策 139 / 108）。第 2 轮（重试后）写正解，
 * 闸门转绿、流程继续推进到 `pending(merge_approval)`。
 */
export function failingGateRounds(taskId: string): NodeScript {
  return {
    ...designRounds(),
    ...implementationRounds(taskId),
    [NODE.developEx]: [
      // 第 1 轮：实现错误 → 闸门真失败
      [
        writeFile(
          'src/lib.js',
          'function add(a, b) { return a - b; }\nmodule.exports = { add };\n',
        ),
        runCommand(
          `git add -A && git -c user.name=e2e -c user.email=e2e@localhost commit -m 'feat(wip): task ${taskId}'`,
        ),
        submit(CodeChanges(taskId)),
      ],
      // 第 2 轮：修好 → 闸门通过
      [
        writeFile(
          'src/lib.js',
          'function add(a, b) { return a + b; }\nmodule.exports = { add };\n',
        ),
        runCommand(
          `git add -A && git -c user.name=e2e -c user.email=e2e@localhost commit -m 'fix: task ${taskId}'`,
        ),
        submit(CodeChanges(taskId)),
      ],
    ],
  };
}

/** 全流程通过脚本：推进到 merge 阶段 A 末尾的 `pending(merge_approval)`。 */
export function fullPassScript(taskId: string): NodeScript {
  return { ...designRounds(), ...implementationRounds(taskId) };
}

/**
 * 人工评审流（主流程票 06，`review_mode = human`）。
 *
 * review 阶段的 agent 只做预审（写报告），validate_output 后系统挂
 * `pending(human_review)` 等人。devEx / reviewEx 各备 **2 轮**：reject 打回 develop
 * 后两节点重入各消费下一轮。审批通过 → test → merge_approval。
 */
export function humanReviewRounds(taskId: string): NodeScript {
  const impl = implementationRounds(taskId);
  return {
    ...designRounds(),
    [NODE.developEx]: [impl[NODE.developEx][0], impl[NODE.developEx][0]],
    [NODE.reviewEx]: [impl[NODE.reviewEx][0], impl[NODE.reviewEx][0]],
    [NODE.testEx]: impl[NODE.testEx],
  };
}

/**
 * 「同项目另一任务」的设计脚本（主流程票 09 用例③）：与 {@link designRounds} 逐字段
 * 相同，**只有 `affected_files` 不同**。
 *
 * 为什么必须不同：冲突检测（决策 53 / 71）比的是**设计文档声明的**
 * `affected_files` / `new_symbols`，不是实际写了哪些文件。两个任务都声明
 * `src/lib.rs` 会在 architect.execute 判定 High 冲突 → 先到者让后到者挂
 * `conflict_wait`（决策 102），用例③ 就测不到基准前移（决策 96）了——那是
 * 冲突检测在正确工作，不是我们要验的路径。
 *
 * 声明与实际写入的偏差在此无害：两个任务都得写 `src/lib.js`，否则闸门
 * `npm test` 必然失败（fixture 的 `add` 还是未实现占位）。
 */
export function siblingDesignRounds(): NodeScript {
  return {
    ...designRounds(),
    [NODE.archEx]: [
      [
        writeFile('design.md', '# 设计\n## 验收标准\n- AC-1 能登录\n'),
        submit(
          ArchitectExecute({
            affectedFiles: ['src/extra.js'],
            acceptanceCriteria: [{ id: 'AC-1', description: '能登录' }],
            designDocPath: 'design.md',
          }),
        ),
      ],
    ],
  };
}

/**
 * 「同项目另一任务」脚本（主流程票 09 用例③，基准前移）：设计声明与主任务错开
 * （见 {@link siblingDesignRounds}），实现上把「自己独有的变更」放在 `src/extra.js`，
 * 而非主任务的 `tests/acceptance.js`。
 *
 * 为什么实现也要错开文件：用例③ 让本任务先合入、主任务随后合入。主任务 rebase 到
 * 新基准后，与基准内容相同的文件不再出现在 diff 里——两个任务若写同一份文件，
 * 主任务的 diff 会退化成**空差异**，阶段 A 按闸门失败分流
 * （`merge_phase_a_inner` 的空 diff 分支），用例就观察不到「approval 被重置后
 * 重走阶段 A」。错开文件集后主任务的 diff 仍有自有的 `tests/acceptance.js`，
 * 断言才落在业务语义上。
 */
export function siblingImplementationRounds(taskId: string): NodeScript {
  return {
    [NODE.developEx]: [
      [
        writeFile(
          'src/lib.js',
          'function add(a, b) { return a + b; }\nmodule.exports = { add };\n',
        ),
        writeFile('src/extra.js', "module.exports = { marker: 'sibling' };\n"),
        runCommand(
          `git add -A && git -c user.name=e2e -c user.email=e2e@localhost commit -m 'feat: task ${taskId}'`,
        ),
        submit(CodeChanges(taskId)),
      ],
    ],
    [NODE.reviewEx]: [
      [
        writeFile('review-report.md', '# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n'),
        submit(ReviewResult(true)),
      ],
    ],
    [NODE.testEx]: [
      [writeFile('test-report.md', '# 测试报告\n全部通过\n'), submit(TestResult(true))],
    ],
  };
}

/** 与 {@link siblingImplementationRounds} 配套的完整脚本（推进到 merge_approval）。 */
export function siblingPassScript(taskId: string): NodeScript {
  return { ...siblingDesignRounds(), ...siblingImplementationRounds(taskId) };
}

/**
 * 合并「返回修改」流（主流程票 06，agent 评审）。
 *
 * `merge_approval` 的 return 打回 develop.execute——三实现阶段各备 **2 轮**：
 * 第 1 轮推进到 merge_approval，return 后重入消费第 2 轮，再次到 merge_approval 后合入。
 */
export function mergeReturnRounds(taskId: string): NodeScript {
  const impl = implementationRounds(taskId);
  return {
    ...designRounds(),
    [NODE.developEx]: [impl[NODE.developEx][0], impl[NODE.developEx][0]],
    [NODE.reviewEx]: [impl[NODE.reviewEx][0], impl[NODE.reviewEx][0]],
    [NODE.testEx]: [impl[NODE.testEx][0], impl[NODE.testEx][0]],
  };
}

/**
 * 可观测性脚本（主流程票 07）：在 fullPass 之上给 review.execute 的报告轮
 * 加一条 `text` 步骤——会话页签要有**可断言的模型文本**，而不是只有工具调用。
 */
export function observabilityRounds(taskId: string): NodeScript {
  const impl = implementationRounds(taskId);
  return {
    ...designRounds(),
    ...impl,
    [NODE.reviewEx]: [
      [
        writeFile('review-report.md', '# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n'),
        submit(ReviewResult(true)),
        // text 放 submit 之后：mock 一请求一步，finish=stop 的纯文本会结束工具循环，
        // 放在前面会让「未找到结构化元数据」（票 07 实测）
        text('评审要点标记-obs7：边界用例覆盖充分，可以进入测试阶段。'),
      ],
    ],
  };
}

/**
 * 入口阻塞脚本（主流程票 09 用例①）：architect.validate_input 报 readiness=false
 * ——任务挂起在流水线第一步，等用户决策（goto / skip），天然充当「停在 pending 的一方」。
 */
export function archBlockerRounds(): NodeScript {
  return {
    ...designRounds(),
    [NODE.archVI]: [[submit(ValidateInput(false, ['任务信息不足：缺验收口径-mark-09a']))]],
  };
}

/**
 * 并行双分支阻塞脚本（主流程票 09 用例②）：architect 通过、游标分裂（决策 90）后，
 * develop-design 与 test-design 的 validate_input **双双**报 blockers——
 * 任务同时存在两个 pending 分支游标，供断言决策 91 的分支分组 UI。
 */
export function parallelBlockerRounds(): NodeScript {
  return {
    ...designRounds(),
    [NODE.devDesignVI]: [[submit(ValidateInput(false, ['dev 设计文档缺依赖注入说明-mark-09b']))]],
    [NODE.testDesignVI]: [[submit(ValidateInput(false, ['tst 缺性能场景-mark-09c']))]],
  };
}

/* ─────────────────────────────── 值班长 / 对讲台（票 03 / 04）─────────────────────────────── */

/**
 * 值班长的脚本槽（对应 testkit 的 `Script::for_foreman()`）：**按「轮」投喂**，
 * 一轮 = 一次回话。
 *
 * 与节点脚本的区别在 mock 的轮判定上：节点用 `messages.length <= 2`（system + user）
 * 认「新一轮节点运行」，而值班长的每轮请求都带同一段态势快照前言 + 历史对话，
 * 数量不固定，故 mock 改按「最后一条消息是 user」认新轮——同一次回话里的工具往返
 * 以 tool 结尾，不会被误判成新轮（否则一次查台账就吃掉下一轮的步骤）。
 */
export const FOREMAN = 'foreman';

/** 值班长的两个只读工具（`FOREMAN_TOOLS`）：与真实人格同一张表，多一个都不给。 */
export const readTask = (taskId: string): Step => tool('read_task', { task_id: taskId });
export const readConversation = (taskId: string, runId?: number): Step =>
  tool('read_conversation', { task_id: taskId, run_id: runId ?? null });

/** 值班长脚本，可直接铺进 NodeScript：`{ ...designRounds(), ...foremanScript([[text('…')]]) }`。 */
export function foremanScript(rounds: Step[][]): NodeScript {
  return { [FOREMAN]: rounds };
}
