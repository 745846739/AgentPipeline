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

/** 实现三阶段（develop / review / test）：真写代码 + 真 git 提交。 */
export function implementationRounds(taskId: string): NodeScript {
  return {
    [NODE.developEx]: [
      [
        writeFile(
          'src/lib.rs',
          'pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n',
        ),
        writeFile('tests/acceptance.rs', '#[test]\nfn ok() {}\n'),
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

/** 全流程通过脚本：推进到 merge 阶段 A 末尾的 `pending(merge_approval)`。 */
export function fullPassScript(taskId: string): NodeScript {
  return { ...designRounds(), ...implementationRounds(taskId) };
}
