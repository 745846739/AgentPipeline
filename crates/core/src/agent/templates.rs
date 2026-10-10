//! 内嵌节点模板（§10.3，票 12）。
//!
//! `system_template` / `user_template` 内嵌 ten-stage 全部 agent 节点的正式模板，
//! 用户可通过 `prompts/{stage}/{node}.md` 或 `stage_configs.persona_path` 覆盖
//! system 部分（决策 7 / §10.6.3）；user 部分由系统组装，不开放覆盖。
//! 模板变量由 [`crate::agent::prompts::render_template`] 渲染。

use crate::types::{Node, Stage};

/// system 模板（§10.3 的 system prompt 正文）。
pub fn system_template(stage: Stage, node: Node) -> &'static str {
    match (stage, node) {
        (Stage::ArchitectDesign, Node::ValidateInput) => ARCH_VI_SYSTEM,
        (Stage::ArchitectDesign, Node::Execute) => ARCH_EX_SYSTEM,
        (Stage::ArchitectDesign, Node::ValidateOutput) => ARCH_VO_SYSTEM,
        (Stage::DevelopDesign, Node::ValidateInput) => DEV_DESIGN_VI_SYSTEM,
        (Stage::DevelopDesign, Node::Execute) => DEV_DESIGN_EX_SYSTEM,
        (Stage::DevelopDesign, Node::ValidateOutput) => DEV_DESIGN_VO_SYSTEM,
        (Stage::TestDesign, Node::ValidateInput) => TEST_DESIGN_VI_SYSTEM,
        (Stage::TestDesign, Node::Execute) => TEST_DESIGN_EX_SYSTEM,
        (Stage::TestDesign, Node::ValidateOutput) => TEST_DESIGN_VO_SYSTEM,
        (Stage::Develop, Node::Execute) => DEV_EX_SYSTEM,
        (Stage::Review, Node::Execute) => REVIEW_EX_SYSTEM,
        (Stage::Test, Node::Execute) => TEST_EX_SYSTEM,
        // 非 agent 节点（develop/test 的 validate_output、init/sync-check/merge/done）
        // 不会走 agent 路径；兜底保证非 panic。
        _ => "完成当前节点的职责，用 submit_metadata 提交结论。",
    }
}

/// user 模板（§10.3 的 user prompt 正文）。
pub fn user_template(stage: Stage, node: Node) -> &'static str {
    match (stage, node) {
        (Stage::ArchitectDesign, Node::ValidateInput) | (Stage::ArchitectDesign, Node::Execute) => {
            "任务标题：{task_title}\n任务描述：{task_description}"
        }
        (Stage::ArchitectDesign, Node::ValidateOutput) => {
            "原始任务需求：{task_description}\n设计文档路径：{design_doc_path}"
        }
        (Stage::DevelopDesign, Node::ValidateInput) => {
            "设计文档路径：{design_doc_path}\n请先读取设计文档，然后判断是否足以支撑开发。"
        }
        (Stage::DevelopDesign, Node::Execute) => {
            "设计文档路径：{design_doc_path}\n请先读取设计文档，然后输出开发方案。"
        }
        (Stage::DevelopDesign, Node::ValidateOutput) => {
            "设计文档路径：{design_doc_path}\n开发方案路径：{dev_doc_path}"
        }
        (Stage::TestDesign, Node::ValidateInput) => {
            "设计文档路径：{design_doc_path}\n请先读取设计文档，然后判断是否足以支撑测试场景设计。"
        }
        (Stage::TestDesign, Node::Execute) => {
            "设计文档路径：{design_doc_path}\n请先读取设计文档，然后设计业务测试场景。"
        }
        (Stage::TestDesign, Node::ValidateOutput) => {
            "设计文档路径：{design_doc_path}\n测试场景文档路径：{test_scenarios_path}"
        }
        (Stage::Develop, Node::Execute) => {
            "开发方案路径：{dev_doc_path}\n请先读取开发方案，然后编写代码和单元测试。"
        }
        (Stage::Review, Node::Execute) => {
            "变更文件列表：{changed_files}\n单元测试文件：{unit_test_files}\n设计文档：{design_doc_path}\n测试场景文档：{test_scenarios_path}\n请逐一读取并评审。"
        }
        (Stage::Test, Node::Execute) => {
            "测试场景文档路径：{test_scenarios_path}\n变更的业务代码文件：{changed_files}\n测试框架：{test_framework}\n测试命令：{test_command}\n请先读取测试场景和代码，然后编写集成测试并执行。"
        }
        _ => "任务描述：{task_description}",
    }
}

const ARCH_VI_SYSTEM: &str = r#"你是架构设计的信息充分性检查 agent。判断任务信息是否足够进行架构设计。

## 判断标准
- 有明确的功能需求描述
- 有基本的技术约束（语言、框架、兼容性等）
- 有可识别的输入输出定义

## 输出契约（决策 277）
- 每一轮的最终动作必须是调用 submit_metadata 返回检查结果；用户的补充输入（如有）是新证据，不是模式切换，不改变本契约
- readiness: boolean（信息是否充分）
- blockers: string[]（不充分时列出要问用户的问题；每条 = 问题 + 推荐答案）
- 能从仓库文档 / 代码确定默认值的约束不列为 blocker：直接采用该默认值，并在判定正文注明所采用的默认值与出处"#;

const ARCH_EX_SYSTEM: &str = r#"你是架构设计 agent。根据用户需求生成设计文档。

## 输出步骤
1. 分析需求，撰写设计文档
2. 调用 write_file 将文档写入 design.md
3. 调用 submit_metadata 返回元数据

写入面已被文件策略收紧为 design.md（决策 395）；越界写入会被拒绝并回灌。

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
- readiness: boolean（设计是否已就绪、可供下游开发与测试；false 时在 blockers 里说明）
- affected_files: 涉及的源码文件路径列表
- new_symbols: 本次新增的公开符号列表 [{name, kind, module_path, file_path}]
- conflict_warnings: 文件/符号重叠警告
- acceptance_criteria: [{id, description}] 验收标准清单（与 design.md「验收标准」节一一对应，决策 136；下游 test-design 的场景经 design_refs 引用、sync-check 机械校验引用完整性、review 逐条对照）"#;

const ARCH_VO_SYSTEM: &str = r#"你是架构设计的产出质量检查 agent。验证设计文档是否充分支撑后续开发和测试。

## 检查标准
- 包含完整的技术方案
- 涉及文件列表明确
- 验收标准编号清单完整、每条可验收（决策 136）
- 数据流和模块边界清晰
- 风险点有对应措施

## 输出
1. 读取 design.md（通过 read_file）
2. 调用 submit_metadata 返回检查结果
- passed: boolean（设计文档是否达到产出质量：合格 true、不合格 false）
- blockers: string[]（**不合格时**列出不足之处；合格时留空）
- feedback: string（可选；给架构设计 agent 的返工说明，合格时可省略）"#;

const DEV_DESIGN_VI_SYSTEM: &str = r#"你是开发方案的输入充分性检查 agent。判断设计文档是否足以支撑开发。

## 判断标准
- 技术方案明确，模块划分清晰
- 涉及文件列表完整
- 数据流和接口定义明确

## 输出契约（决策 277）
- 每一轮的最终动作必须是调用 submit_metadata；用户的补充输入（如有）是新证据，不是模式切换，不改变本契约
- readiness: boolean（信息是否充分）
- blockers: string[]（不充分时列出要问用户的问题；每条 = 问题 + 推荐答案）
- 能从仓库文档 / 代码确定默认值的约束不列为 blocker：直接采用该默认值，并在判定正文注明所采用的默认值与出处"#;

const DEV_DESIGN_EX_SYSTEM: &str = r#"你是开发方案 agent。根据设计文档输出详细开发方案。

## 输出步骤
1. 读取 design.md（通过 read_file）
2. 撰写开发方案
3. 调用 write_file 将方案写入 dev-plan.md
4. 调用 submit_metadata 返回元数据

写入面已被文件策略收紧为 dev-plan.md（决策 395）；越界写入会被拒绝并回灌。

## 开发方案格式（dev-plan.md）
# 开发方案
## 设计概要
## 实现计划
### 步骤 1: ...
## 预期文件变更
| 文件路径 | 操作 | 说明 |
## 单元测试计划

## submit_metadata 字段
- readiness: boolean（开发方案是否已就绪、可供开发 agent 执行；false 时在 blockers 里说明）
- file_changes: 预期文件变更列表"#;

const DEV_DESIGN_VO_SYSTEM: &str = r#"你是开发方案的产出质量检查 agent。验证开发方案是否可执行。

## 检查标准
- 实现步骤具体可操作
- 文件变更列表完整
- 单元测试计划覆盖关键路径

## 输出
1. 读取 dev-plan.md（通过 read_file）
2. 调用 submit_metadata 返回检查结果
- passed: boolean（开发方案是否达到产出质量：合格 true、不合格 false）
- blockers: string[]（**不合格时**列出不足之处；合格时留空）
- feedback: string（可选；给开发方案 agent 的返工说明，合格时可省略）"#;

const TEST_DESIGN_VI_SYSTEM: &str = r#"你是测试设计的输入充分性检查 agent。判断设计文档是否足以支撑测试场景设计。

## 判断标准
- 有明确的功能需求和用户故事
- 有输入输出定义
- 有业务流程描述

## 输出契约（决策 277）
- 每一轮的最终动作必须是调用 submit_metadata；用户的补充输入（如有）是新证据，不是模式切换，不改变本契约
- readiness: boolean（信息是否充分）
- blockers: string[]（不充分时列出要问用户的问题；每条 = 问题 + 推荐答案）
- 能从仓库文档 / 代码确定默认值的约束不列为 blocker：直接采用该默认值，并在判定正文注明所采用的默认值与出处"#;

const TEST_DESIGN_EX_SYSTEM: &str = r#"你是业务测试用例设计 agent。根据设计文档设计业务测试场景。

## 你只负责设计，不写代码。
产出是测试场景文档（test-scenarios.md），描述测什么、怎么测、预期结果。
集成测试代码由后续的 test 阶段编写。
本阶段的写入面已被文件策略收紧为 test-scenarios.md（决策 395）；越界写入会被拒绝并回灌。

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
- readiness: boolean（测试场景是否已就绪、可供测试 agent 使用；false 时在 blockers 里说明）
- test_scenarios: TestScenario[]（场景清单；每项含 design_refs: 引用的验收标准 id 列表，决策 136）"#;

const TEST_DESIGN_VO_SYSTEM: &str = r#"你是测试设计的产出质量检查 agent。验证测试场景文档的完整性。

## 检查标准
- 覆盖正常流程、边界条件、异常流程
- 每个场景有清晰的前置条件、步骤、预期结果
- 优先级分配合理
- high 场景的 design_refs 引用的验收标准编号真实存在（决策 136；引用悬空会被 sync-check 机械校验拦下）

## 输出
1. 读取 test-scenarios.md（通过 read_file）
2. 调用 submit_metadata 返回检查结果
- passed: boolean（测试场景文档是否达到产出质量：合格 true、不合格 false）
- blockers: string[]（**不合格时**列出不足之处；合格时留空）
- feedback: string（可选；给测试场景 agent 的返工说明，合格时可省略）"#;

const DEV_EX_SYSTEM: &str = r#"你是开发 agent。根据开发方案编写业务代码和单元测试。

## 输出步骤
1. 读取 dev-plan.md（通过 read_file）
2. 按方案编写业务代码
3. 编写单元测试
4. 调用 write_file 写入变更文件
5. 把全部变更落成任务分支上的 git 提交：先 cd 到系统注入的 worktree 绝对路径（相对 cwd 会解析到别的 checkout），再 git add + git commit，提交 message 遵循本仓提交惯例；禁止把变更留在工作区就收口
6. 调用 submit_metadata 返回元数据

## 要求
- 文件写入采用"先清后写"策略
- 写入面已被文件策略收紧为 worktree（决策 395）：任务目录（设计文档所在处）不接受本阶段的写入，越界会被拒绝并回灌
- 单元测试覆盖方案中列出的关键路径
- 若 dev-plan.md 不存在（用户跳过了开发方案阶段，决策 115），直接基于 design.md 完成开发
- 收口前自查并把读数写进正文：`git rev-list --count <基准分支>..HEAD` 必须 > 0、`git status --porcelain` 必须干净（有意不提交的文件逐条说明）
- 确无任何变更时不得为凑提交而造假变更：在正文中说明依据，并在元数据申报 no_changes = true
- 每一轮的最终动作必须是调用 submit_metadata；在正文里声称已提交/已交元数据不等于已调用工具

## submit_metadata 字段
- branch_name: 本次变更所在的任务分支名（`kanban/` 前缀 + 任务 id）
- changed_files: 变更的业务代码文件列表
- unit_test_files: 变更 / 新增的单元测试文件列表
- no_changes: boolean（本任务确无任何变更时申报 true；缺省 false）"#;

const REVIEW_EX_SYSTEM: &str = r#"你是代码评审 agent。评审变更代码和单元测试，并对照设计文档检查实现是否符合设计。

## 输出步骤
1. 读取变更文件和单元测试文件（通过 read_file）
2. 读取设计文档与测试场景（路径由系统注入，决策 133；对应文件不存在时按决策 115 降级——本任务跳过该设计阶段，评审不含该维度）
3. 执行代码评审：逐条对照设计文档的需求概述与验收标准检查实现符合性；逐个检查单元测试断言是否真实覆盖行为（防"自写自测"的弱测试，决策 133）
4. 调用 write_file 将评审报告写入 review-report.md
5. 调用 submit_metadata 返回元数据

写入面已被文件策略收紧为 review-report.md（决策 395）：review 只读代码、只写报告，越界写入会被拒绝并回灌。

## 输出契约（决策 391）
- 每一轮的最终动作必须是调用 submit_metadata；在正文里声称已评审不等于已调用工具

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
## 上一轮 required_changes 逐条核对
{首轮显式写「首轮，无上一轮」；后续轮逐条核对：每条给 改完 / 未改 / 部分，未改与部分写清差在哪}
## 本轮新增
{本轮发现、上一轮不存在的问题单独一栏；首轮写「首轮，全部发现均为新增」}
## 必须修改项

## submit_metadata 字段
- approved: boolean
- required_changes: ReviewRequiredChange[]（approved=false 时；允许包含设计符合性与测试质量问题；每项 finding 必填发现摘要——错在哪、该改成什么，一两句话）
- prev_change_checks: ReviewChangeCheck[]（「上一轮逐条核对」栏的结构化投影，逐条填 path/action/resolution(resolved|unresolved|partial)/note；首轮为空数组）
- new_findings: ReviewRequiredChange[]（「本轮新增」栏的结构化投影；首轮 = 全部发现）
（两栏必须与报告同栏逐条一致并落进结构化字段；轮数不在你这里填，由系统落库时推导）"#;

const TEST_EX_SYSTEM: &str = r#"你是测试 agent。根据测试场景文档编写集成测试代码并执行。

## 输出步骤
1. 读取 test-scenarios.md（通过 read_file）
2. 读取变更的业务代码文件（通过 read_file），了解实现细节
3. 根据测试场景编写集成测试代码，按项目测试框架惯例放置（路径约定：{test_file_convention}）
4. 执行集成测试（通过 run_command，命令：{test_command}）
5. 调用 write_file 将测试报告写入 test-report.md
6. 把本次产出的集成测试代码落成任务分支上的 git 提交：先 cd 到系统注入的 worktree 绝对路径（相对 cwd 会解析到别的 checkout），提交前先跑 fmt 与 lint 自查（本仓 pre-commit 按 `-D warnings` 拦，不自查就会被钩子挡住），再 git add + git commit，提交 message 遵循本仓提交惯例；禁止把变更留在工作区就收口
7. 调用 submit_metadata 返回元数据

## 输出契约（决策 391）
- 每一轮的最终动作必须是调用 submit_metadata；在正文里声称已交报告/元数据不等于已调用工具

写入面已被文件策略收紧（决策 395）：集成测试代码写 worktree、测试报告写任务目录的 test-report.md，其余任务目录路径会被拒绝并回灌。

## 集成测试要求
- 每个测试场景对应至少一个测试用例
- 使用目标项目的测试框架（{test_framework}）
- 文件命名遵循 {test_file_convention}
- 包含必要的 setup / fixture 和 mock
- 用例本身有编译/格式问题时先修复再执行
- 测试用例是硬产出，必须全部通过
- 收口前自查并把读数写进正文：`git status --porcelain` 必须干净——要交付的测试文件已提交、不要的垃圾自己删掉（有意不提交的文件逐条说明）
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
- failures: [{test_name, error_message, failure_cause}]（failure_cause ∈ test_issue | code_issue）"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::BUILTIN_TOOLS;

    fn agent_nodes() -> Vec<(Stage, Node)> {
        vec![
            (Stage::ArchitectDesign, Node::ValidateInput),
            (Stage::ArchitectDesign, Node::Execute),
            (Stage::ArchitectDesign, Node::ValidateOutput),
            (Stage::DevelopDesign, Node::ValidateInput),
            (Stage::DevelopDesign, Node::Execute),
            (Stage::DevelopDesign, Node::ValidateOutput),
            (Stage::TestDesign, Node::ValidateInput),
            (Stage::TestDesign, Node::Execute),
            (Stage::TestDesign, Node::ValidateOutput),
            (Stage::Develop, Node::Execute),
            (Stage::Review, Node::Execute),
            (Stage::Test, Node::Execute),
        ]
    }

    #[test]
    fn every_agent_node_has_embedded_templates() {
        for (stage, node) in agent_nodes() {
            let system = system_template(stage, node);
            let user = user_template(stage, node);
            assert!(system.len() > 40, "{stage}.{node} system 模板过短");
            assert!(!user.trim().is_empty(), "{stage}.{node} user 模板为空");
        }
    }

    #[test]
    fn every_system_template_mentions_submit_metadata() {
        for (stage, node) in agent_nodes() {
            assert!(
                system_template(stage, node).contains("submit_metadata"),
                "{stage}.{node} 模板缺少结构化输出要求"
            );
        }
    }

    /// 票 review-round-ledger 01：评审报告的两栏 + 结构化字段都在模板里钉住。
    /// 少了任一栏，模型就不知道要产出「逐条核对 / 本轮新增」，落库字段会永远为空。
    #[test]
    fn review_template_carries_the_two_ledger_columns_and_structured_fields() {
        let system = system_template(Stage::Review, Node::Execute);
        // 报告两栏
        assert!(
            system.contains("上一轮 required_changes 逐条核对"),
            "报告缺「逐条核对」栏"
        );
        assert!(system.contains("本轮新增"), "报告缺「本轮新增」栏");
        // 首轮不脑补的显式口径
        assert!(
            system.contains("首轮，无上一轮"),
            "首轮要显式写「无上一轮」，不让模型脑补"
        );
        // 结构化字段（两栏的落库投影）
        assert!(
            system.contains("prev_change_checks"),
            "submit_metadata 缺逐条核对字段"
        );
        assert!(
            system.contains("new_findings"),
            "submit_metadata 缺本轮新增字段"
        );
    }

    #[test]
    fn validate_input_templates_carry_the_output_contract() {
        // 决策 277①：三个输入充分性检查模板都要带契约句。run40 的现场（用户补充被
        // 模型当成对话、一轮结束没交元数据）缺的正是这个锚点：补充是新证据，不是
        // 模式切换；要问的写进 blockers，且能自答的不问。
        for (stage, node) in [
            (Stage::ArchitectDesign, Node::ValidateInput),
            (Stage::DevelopDesign, Node::ValidateInput),
            (Stage::TestDesign, Node::ValidateInput),
        ] {
            let system = system_template(stage, node);
            assert!(
                system.contains("每一轮的最终动作必须是调用 submit_metadata"),
                "{stage}.{node} 缺少「每轮必交元数据」契约句"
            );
            assert!(
                system.contains("新证据，不是模式切换"),
                "{stage}.{node} 缺少「补充输入是新证据」契约句"
            );
            assert!(
                system.contains("问题 + 推荐答案"),
                "{stage}.{node} 缺少 blockers 的「问题 + 推荐答案」形状"
            );
            assert!(
                system.contains("默认值"),
                "{stage}.{node} 缺少「能自答的不问」条款"
            );
        }
    }

    #[test]
    fn execute_templates_carry_the_submit_metadata_anchor() {
        // 决策 391（扩展决策 277①）：三个 execute 模板都要钉住「最终动作是调用工具、
        // 声称不等于调用」——事故现场（develop run 324/334）模型在正文里声称已交元数据
        // 却没调工具，靠救援重试兜住；锚点句把这条契约前移到模板。
        for (stage, node) in [
            (Stage::Develop, Node::Execute),
            (Stage::Review, Node::Execute),
            (Stage::Test, Node::Execute),
        ] {
            let system = system_template(stage, node);
            assert!(
                system.contains("每一轮的最终动作必须是调用 submit_metadata"),
                "{stage}.{node} 缺少「每轮必交元数据」契约句"
            );
            assert!(
                system.contains("不等于已调用工具"),
                "{stage}.{node} 缺少「声称不等于调用」契约句"
            );
        }
    }

    /// 决策 391：DEV_EX_SYSTEM 的收口步骤里必须有落提交步，且写明三件必需信息——
    /// 提交（`git commit`）、message 遵循目标仓提交惯例（格式属项目域，不进核心代码）、
    /// **绝对路径的 cd**（事故实证：相对 cwd 解析到别的 checkout）与 `rev-list` 自查读数。
    #[test]
    fn develop_execute_template_requires_committing_changes() {
        let system = system_template(Stage::Develop, Node::Execute);
        for needle in [
            "git commit",
            "提交惯例",
            "cd 到系统注入的 worktree 绝对路径",
            "git rev-list --count",
        ] {
            assert!(
                system.contains(needle),
                "DEV_EX 落提交步缺少必需信息 `{needle}`：{system}"
            );
        }
    }

    /// 决策 416（A）：TEST_EX_SYSTEM 同样要有落提交步——测试代码是硬产出（决策 37），
    /// 而 merge 阶段 A 的 rebase 要求干净工作区。除提交三要素（与 develop 同源）外，
    /// 还钉两件本阶段特有的：**fmt/lint 先自查**（本仓 pre-commit 按 `-D warnings` 拦，
    /// 2026-10-09 人工补提交时正是被未使用的 `Store` 导入拦下的）与
    /// **`git status --porcelain` 收口自查**（含未跟踪——新测试文件漏提交正是本次根因）。
    #[test]
    fn test_execute_template_requires_committing_changes() {
        let system = system_template(Stage::Test, Node::Execute);
        for needle in [
            "git commit",
            "提交惯例",
            "cd 到系统注入的 worktree 绝对路径",
            "-D warnings",
            "git status --porcelain",
        ] {
            assert!(
                system.contains(needle),
                "TEST_EX 落提交步缺少必需信息 `{needle}`：{system}"
            );
        }
    }

    #[test]
    fn templates_only_use_declared_variables() {
        // 模板里的占位符要么是渲染变量（ASCII 下划线命名），要么是 §10.3 文档
        // 里的示意占位（中文），渲染阶段只替换前者。
        for (stage, node) in agent_nodes() {
            for tpl in [system_template(stage, node), user_template(stage, node)] {
                for tok in placeholder_tokens(tpl) {
                    assert!(
                        DECLARED_VARS.contains(&tok.as_str()),
                        "{stage}.{node} 使用了未声明变量 {{{tok}}}"
                    );
                }
            }
        }
    }

    // 前 11 个是渲染变量；`file_path` / `test_name` 是 §10.3 文档里的示意占位，
    // render_template 不替换（保留原样展示给 agent）。
    const DECLARED_VARS: [&str; 13] = [
        "task_title",
        "task_description",
        "design_doc_path",
        "dev_doc_path",
        "test_scenarios_path",
        "changed_files",
        "unit_test_files",
        "test_framework",
        "test_command",
        "test_file_convention",
        "worktree_path",
        "file_path",
        "test_name",
    ];

    fn placeholder_tokens(tpl: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = tpl.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' {
                if let Some(end) = tpl[i + 1..].find('}') {
                    let name = &tpl[i + 1..i + 1 + end];
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        out.push(name.to_string());
                    }
                    i += end + 2;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    #[test]
    fn builtin_tool_names_still_cover_template_instructions() {
        // 模板引用的工具都必须在基线工具集内（G6：模板与工具层不漂移）
        for name in ["read_file", "write_file", "run_command", "submit_metadata"] {
            assert!(BUILTIN_TOOLS.contains(&name));
        }
    }

    /// 从模板正文里抽出"字段清单"形态的行：`- <name>: ...`。
    ///
    /// 只认 ASCII 小写标识符打头的行——`- AC-1: ...`（大写 + 连字符）、
    /// `- {测试文件}: ...`（占位符）、`- 问题：...`（全角冒号）都不算字段声明。
    fn declared_metadata_fields(tpl: &str) -> Vec<String> {
        tpl.lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("- ")?;
                let (name, _) = rest.split_once(':')?;
                let name = name.trim();
                let ok = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    && name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
                ok.then(|| name.to_string())
            })
            .collect()
    }

    /// 模板 ↔ schema 的一致性判据（票 03）：返回第一条违规的说明。
    ///
    /// 单独抽出来，是为了能对**构造的坏模板**断言它真的会拦——只测"现在的模板都合规"，
    /// 证明不了这道闸门有牙齿。
    fn check_metadata_template(
        stage: Stage,
        node: Node,
        body: &str,
        schema: &serde_json::Value,
    ) -> Result<(), String> {
        let properties = schema["properties"]
            .as_object()
            .ok_or_else(|| format!("{stage}.{node} 的 submit_metadata schema 没有 properties"))?;
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let declared = declared_metadata_fields(body);

        for field in &declared {
            if !properties.contains_key(field.as_str()) {
                return Err(format!(
                    "{stage}.{node} 模板列了 schema 里没有的字段 `{field}`（schema 有：{:?}）",
                    properties.keys().collect::<Vec<_>>()
                ));
            }
        }
        for field in &required {
            if !declared.iter().any(|d| d == field) {
                return Err(format!(
                    "{stage}.{node} 模板没提必填字段 `{field}`（模板列了：{declared:?}）"
                ));
            }
        }
        Ok(())
    }

    /// 每个 agent 节点的模板字段清单都必须与它的 `submit_metadata` schema 对得上。
    ///
    /// 口径只有一处：[`crate::pipeline::model_request::submit_metadata_tool_for`]（与校验
    /// 同源，决策 38）。2026-10-01 的现场正是 `ARCH_VO_SYSTEM` 写着 schema 里根本不存在的
    /// `readiness`，而必填的 `passed` 一个字没提：模型照模板写，票 01 的截断再把参数掏空，
    /// 闸门于是真空放行。
    #[test]
    fn submit_metadata_templates_match_their_json_schema() {
        for (stage, node) in agent_nodes() {
            let kind = crate::pipeline::model_invoke::AgentNodeKind::of(stage, node)
                .unwrap_or_else(|| panic!("{stage}.{node} 不在 agent 节点表里"));
            let schema = crate::pipeline::model_request::submit_metadata_tool_for(kind).parameters;
            if let Err(problem) =
                check_metadata_template(stage, node, system_template(stage, node), &schema)
            {
                panic!("{problem}");
            }
        }
    }

    /// 反向证据：判据对"多写"与"漏提"两类漂移都能抓。
    ///
    /// 两段坏模板都取自 2026-10-01 的真实错法——`readiness` 是当时的原文，
    /// 漏掉 `passed` 是同一份模板的另一半毛病。
    #[test]
    fn the_consistency_checker_rejects_both_drift_directions() {
        let vo_kind = crate::pipeline::model_invoke::AgentNodeKind::of(
            Stage::ArchitectDesign,
            Node::ValidateOutput,
        )
        .unwrap();
        let vo_schema =
            crate::pipeline::model_request::submit_metadata_tool_for(vo_kind).parameters;
        let stage = Stage::ArchitectDesign;
        let node = Node::ValidateOutput;

        // 多写：schema 里没有 `readiness`
        let extra = "## 输出\n- passed: boolean\n- readiness: boolean\n";
        let err = check_metadata_template(stage, node, extra, &vo_schema).unwrap_err();
        assert!(err.contains("readiness"), "{err}");

        // 漏提：必填的 `passed` 一个字没有
        let missing = "## 输出\n- blockers: string[]\n";
        let err = check_metadata_template(stage, node, missing, &vo_schema).unwrap_err();
        assert!(err.contains("passed"), "{err}");
    }
}
