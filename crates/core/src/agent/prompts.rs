//! System / user prompt 组装（决策 51 / 28 / 7 / 109 / 126 / 138 / 31 / 137）。
//!
//! 段落顺序固定为 `[基线前言][AGENTS.md][persona][格式规则]`——固定前缀保证 prompt cache
//! 稳定命中（§12.13.5）。user prompt 的追加段（gate_recheck / backtrack-feedback /
//! retry-feedback）**首轮为空不渲染**。

use std::path::{Path, PathBuf};

use crate::types::{Node, Stage};

/// 基线前言（不可覆盖）。
pub const BASELINE_PREAMBLE: &str = "你是 AgentPipeline 的节点 agent。严格按输出格式要求调用工具，\
不要臆测文件内容——需要时用 read_file 读取。禁止修改本阶段产出之外的任何文件。";

/// 格式规则段（不可覆盖）。
pub const FORMAT_RULES: &str = "## 输出格式\n\
- 产出文件一律通过 write_file 写入指定路径（覆盖写入）。\n\
- 结构化流转信息一律通过 submit_metadata 提交，不要在正文里夹带 JSON。\n\
- 判断结论必须给出依据，不要只给结论。";

/// AGENTS.md 缺失时的非空默认上下文（决策 51）。
pub fn default_agents_context(
    project_root: &Path,
    language: Option<&str>,
    test_framework: Option<&str>,
) -> String {
    format!(
        "## 项目上下文\n本仓库无 AGENTS.md。\n项目根路径：{}\n语言：{}\n测试框架：{}",
        project_root.display(),
        language.unwrap_or("未知"),
        test_framework.unwrap_or("未知"),
    )
}

/// 加载项目上下文（G3）：优先读 `{project_root}/AGENTS.md`，缺失或为空时
/// 回退到非空默认上下文（决策 51）。
pub fn load_agents_context(
    project_root: &Path,
    language: Option<&str>,
    test_framework: Option<&str>,
) -> String {
    match std::fs::read_to_string(project_root.join("AGENTS.md")) {
        Ok(content) if !content.trim().is_empty() => {
            format!("## 项目上下文（AGENTS.md）\n{}", content.trim())
        }
        _ => default_agents_context(project_root, language, test_framework),
    }
}

/// 组装 system prompt：
/// `[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能清单][格式规则]`。
/// 固定前缀保证 prompt cache 稳定命中（§12.13.5）；worktree / 任务目录是任务级
/// 常量，不破坏同一任务内重试的缓存。
pub fn build_system_prompt(
    agents_context: &str,
    persona: &str,
    workdirs: &str,
    skills: &[String],
) -> String {
    let agents = if agents_context.trim().is_empty() {
        default_agents_context(Path::new("(未提供)"), None, None)
    } else {
        agents_context.to_string()
    };
    let mut out = format!(
        "{BASELINE_PREAMBLE}\n\n## 工作目录\n{workdirs}\n\n{agents}\n\n{}\n\n",
        persona.trim()
    );
    if !skills.is_empty() {
        out.push_str("## 已启用技能\n");
        for s in skills {
            out.push_str(&format!("- {s}\n"));
        }
        out.push('\n');
    }
    out.push_str(FORMAT_RULES);
    out
}

/// persona 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPersona {
    pub content: String,
    /// 是否来自用户覆盖目录（决策 7）。
    pub from_override: bool,
    pub path: Option<PathBuf>,
}

/// 解析 persona：用户 `{home}/prompts/{stage}/{node}.md` 覆盖内嵌默认（决策 7）。
pub fn resolve_persona(
    prompts_dir: &Path,
    stage: Stage,
    node: Node,
    embedded_default: &str,
) -> ResolvedPersona {
    let path = prompts_dir
        .join(stage.prompt_dir())
        .join(format!("{}.md", node.as_str()));
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => ResolvedPersona {
            content,
            from_override: true,
            path: Some(path),
        },
        _ => ResolvedPersona {
            content: embedded_default.to_string(),
            from_override: false,
            path: None,
        },
    }
}

/// 模板变量替换上下文（决策 31 / §10.3 / G12）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateVars {
    /// 目标项目的测试命令（`{test_command}`）。
    pub test_command: String,
    /// 测试文件命名惯例（`{test_file_convention}`）。
    pub test_file_convention: String,
    /// 目标项目测试框架（`{test_framework}`）。
    pub test_framework: String,
    /// worktree 绝对路径（G12：必须显式告知，不允许 agent 猜测）。
    pub worktree_path: String,
    /// 任务目录绝对路径（G12）。
    pub task_dir: String,
    /// 任务标题（`{task_title}`）。
    pub task_title: String,
    /// 任务描述（`{task_description}`）。
    pub task_description: String,
    /// 设计文档绝对路径（`{design_doc_path}`；缺失时为降级说明，决策 115）。
    pub design_doc_path: String,
    /// 开发方案绝对路径（`{dev_doc_path}`）。
    pub dev_doc_path: String,
    /// 测试场景文档绝对路径（`{test_scenarios_path}`）。
    pub test_scenarios_path: String,
    /// 变更文件列表（`{changed_files}`，每行一个路径）。
    pub changed_files: String,
    /// 单元测试文件列表（`{unit_test_files}`，每行一个路径）。
    pub unit_test_files: String,
}

/// 渲染模板变量。
pub fn render_template(template: &str, vars: &TemplateVars) -> String {
    template
        .replace("{test_command}", &vars.test_command)
        .replace("{test_file_convention}", &vars.test_file_convention)
        .replace("{test_framework}", &vars.test_framework)
        .replace("{worktree_path}", &vars.worktree_path)
        .replace("{task_dir}", &vars.task_dir)
        .replace("{task_title}", &vars.task_title)
        .replace("{task_description}", &vars.task_description)
        .replace("{design_doc_path}", &vars.design_doc_path)
        .replace("{dev_doc_path}", &vars.dev_doc_path)
        .replace("{test_scenarios_path}", &vars.test_scenarios_path)
        .replace("{changed_files}", &vars.changed_files)
        .replace("{unit_test_files}", &vars.unit_test_files)
}

/// user prompt 的可选追加段（决策 109 / 126 / 138）。
///
/// 约定：**首轮为空不渲染**——空段不产生额外的标题与空行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSegments {
    /// 闸门复检上下文（决策 109）：`gate_recheck = true` 时非空。
    pub gate_recheck: Option<String>,
    /// sync-check backtrack 的双方 blockers（决策 126）。
    pub backtrack_feedback: Option<String>,
    /// develop / test 重试耗尽的失败摘要（决策 138）。
    pub retry_feedback: Option<String>,
}

/// 组装 user prompt：主模板 + 非空追加段。
pub fn build_user_prompt(main: &str, segments: &PromptSegments) -> String {
    let mut out = main.trim_end().to_string();
    for (title, body) in [
        (
            "## 合入闸门失败复检上下文",
            segments.gate_recheck.as_deref(),
        ),
        ("## 上游回溯反馈", segments.backtrack_feedback.as_deref()),
        ("## 重试历史摘要", segments.retry_feedback.as_deref()),
    ] {
        if let Some(body) = body {
            if !body.trim().is_empty() {
                out.push_str(&format!("\n\n{title}\n{}", body.trim()));
            }
        }
    }
    out
}

/// prompt 版本标注（决策 137）：最终组装 system prompt 的 SHA-256 前 16 位。
pub fn prompt_template_hash(system_prompt: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(system_prompt.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

/// prompt 覆盖目录链路：任务目录之外的 prompt 根（决策 7）。
pub fn prompts_root(home_prompts_dir: &Path, config_dir: Option<&Path>) -> PathBuf {
    config_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| home_prompts_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persona_for(stage: Stage, node: Node) -> &'static str {
        match (stage, node) {
            (Stage::ArchitectDesign, Node::Execute) => "你是架构设计 agent。",
            (Stage::Develop, Node::Execute) => "你是开发 agent，使用 {test_command} 跑测试。",
            _ => "通用 persona。",
        }
    }

    // ── 组装顺序 golden ──

    #[test]
    fn system_prompt_section_order_is_golden() {
        let prompt = build_system_prompt(
            "## 项目上下文\n仓库 X",
            "PERSONA_BODY",
            "worktree：/wt\n任务目录：/td",
            &["rtk".to_string()],
        );
        let i_baseline = prompt.find(BASELINE_PREAMBLE).unwrap();
        let i_workdirs = prompt.find("## 工作目录").unwrap();
        let i_agents = prompt.find("## 项目上下文").unwrap();
        let i_persona = prompt.find("PERSONA_BODY").unwrap();
        let i_skills = prompt.find("## 已启用技能").unwrap();
        let i_format = prompt.find("## 输出格式").unwrap();
        assert!(i_baseline < i_workdirs);
        assert!(i_workdirs < i_agents);
        assert!(i_agents < i_persona);
        assert!(i_persona < i_skills);
        assert!(i_skills < i_format);
    }

    #[test]
    fn system_prompt_without_skills_has_no_skills_section() {
        let prompt = build_system_prompt("ctx", "persona", "worktree：/wt", &[]);
        assert!(!prompt.contains("## 已启用技能"));
    }

    #[test]
    fn system_prompt_snapshot() {
        let prompt = build_system_prompt(
            "## 项目上下文\n仓库：/repo\n语言：Rust\n测试框架：cargo",
            "你是架构设计 agent。",
            "worktree：/home/u/.agentpipeline/worktrees/t1\n任务目录：/home/u/.agentpipeline/tasks/t1",
            &[],
        );
        insta::assert_snapshot!("system_prompt", prompt);
    }

    #[test]
    fn missing_agents_md_injects_non_empty_default() {
        let prompt = build_system_prompt("", "persona", "worktree：/wt", &[]);
        assert!(prompt.contains("本仓库无 AGENTS.md"));
        assert!(prompt.contains("项目根路径："));
        // 默认上下文里的字段有兜底，不会是空串
        assert!(!default_agents_context(Path::new("/repo"), None, None)
            .trim()
            .is_empty());
        assert!(
            default_agents_context(Path::new("/repo"), Some("Rust"), Some("cargo"))
                .contains("测试框架：cargo")
        );
    }

    // ── AGENTS.md 加载（G3）──

    #[test]
    fn agents_md_content_becomes_agents_context() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("AGENTS.md"),
            "# 项目约定\n- 用 just test 跑测试",
        )
        .unwrap();
        let ctx = load_agents_context(tmp.path(), Some("Rust"), Some("cargo"));
        assert!(ctx.starts_with("## 项目上下文（AGENTS.md）"));
        assert!(ctx.contains("# 项目约定"));
        assert!(ctx.contains("just test"));
    }

    #[test]
    fn missing_or_blank_agents_md_falls_back_to_default() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = load_agents_context(tmp.path(), Some("Rust"), None);
        assert!(ctx.contains("本仓库无 AGENTS.md"));

        std::fs::write(tmp.path().join("AGENTS.md"), "   \n").unwrap();
        assert!(load_agents_context(tmp.path(), None, None).contains("本仓库无 AGENTS.md"));
    }

    // ── prompts/ 覆盖生效 ──

    #[test]
    fn prompts_dir_override_wins_over_embedded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("architect_design");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("execute.md"), "覆盖后的 persona").unwrap();

        let got = resolve_persona(
            tmp.path(),
            Stage::ArchitectDesign,
            Node::Execute,
            "内嵌 persona",
        );
        assert!(got.from_override);
        assert_eq!(got.content, "覆盖后的 persona");
        assert!(got.path.unwrap().ends_with("architect_design/execute.md"));
    }

    #[test]
    fn embedded_persona_used_when_no_override() {
        let tmp = tempfile::tempdir().unwrap();
        let got = resolve_persona(tmp.path(), Stage::Develop, Node::Execute, "内嵌 persona");
        assert!(!got.from_override);
        assert_eq!(got.content, "内嵌 persona");
        assert!(got.path.is_none());
    }

    #[test]
    fn empty_override_file_falls_back_to_embedded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("develop");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("execute.md"), "   \n").unwrap();
        let got = resolve_persona(tmp.path(), Stage::Develop, Node::Execute, "内嵌 persona");
        assert!(!got.from_override);
        assert_eq!(got.content, "内嵌 persona");
    }

    #[test]
    fn prompt_dir_mapping_matches_layout() {
        assert_eq!(Stage::ArchitectDesign.prompt_dir(), "architect_design");
        assert_eq!(Stage::DevelopDesign.prompt_dir(), "develop_design");
        assert_eq!(Stage::TestDesign.prompt_dir(), "test_design");
        assert_eq!(Stage::Develop.prompt_dir(), "develop");
        assert_eq!(Stage::Review.prompt_dir(), "review");
        assert_eq!(Stage::Test.prompt_dir(), "test");
    }

    // ── 模板变量 ──

    #[test]
    fn template_variables_are_substituted() {
        let vars = TemplateVars {
            test_command: "cargo test -- --test-threads=1".into(),
            test_file_convention: "tests/*_test.rs".into(),
            test_framework: "cargo".into(),
            worktree_path: "/home/u/.agentpipeline/worktrees/t1".into(),
            task_dir: "/home/u/.agentpipeline/tasks/t1".into(),
            task_title: "登录功能".into(),
            task_description: "实现登录".into(),
            design_doc_path: "/home/u/.agentpipeline/tasks/t1/design.md".into(),
            dev_doc_path: "/home/u/.agentpipeline/tasks/t1/dev-plan.md".into(),
            test_scenarios_path: "/home/u/.agentpipeline/tasks/t1/test-scenarios.md".into(),
            changed_files: "src/login.rs".into(),
            unit_test_files: "tests/login_test.rs".into(),
        };
        let rendered = render_template(
            "跑 {test_command}；文件放 {test_file_convention}；框架 {test_framework}；\
             工作区 {worktree_path}；产出 {task_dir}；标题 {task_title}；描述 {task_description}；\
             设计 {design_doc_path}；方案 {dev_doc_path}；场景 {test_scenarios_path}；\
             变更 {changed_files}；测试 {unit_test_files}",
            &vars,
        );
        assert!(rendered.contains("cargo test -- --test-threads=1"));
        assert!(rendered.contains("tests/*_test.rs"));
        assert!(rendered.contains("框架 cargo"));
        assert!(rendered.contains("/home/u/.agentpipeline/worktrees/t1"));
        assert!(rendered.contains("/home/u/.agentpipeline/tasks/t1"));
        assert!(rendered.contains("标题 登录功能"));
        assert!(rendered.contains("设计 /home/u/.agentpipeline/tasks/t1/design.md"));
        assert!(rendered.contains("变更 src/login.rs"));
        assert!(rendered.contains("测试 tests/login_test.rs"));
        assert!(!rendered.contains("{test_command}"));
        assert!(!rendered.contains("{design_doc_path}"));
    }

    #[test]
    fn python_and_node_test_commands_render() {
        for (cmd, conv) in [
            ("pytest -q", "tests/test_*.py"),
            ("npm test", "**/*.test.ts"),
        ] {
            let vars = TemplateVars {
                test_command: cmd.into(),
                test_file_convention: conv.into(),
                ..Default::default()
            };
            let out = render_template("{test_command} / {test_file_convention}", &vars);
            assert_eq!(out, format!("{cmd} / {conv}"));
        }
    }

    // ── user prompt 追加段：首轮为空不渲染 ──

    #[test]
    fn empty_segments_render_nothing_on_first_round() {
        let out = build_user_prompt("主 prompt", &PromptSegments::default());
        assert_eq!(out, "主 prompt");
        assert!(!out.contains("##"));
    }

    #[test]
    fn gate_recheck_segment_rendered_when_present() {
        let seg = PromptSegments {
            gate_recheck: Some("失败用例：test_login\n完整日志：/x/out.txt".into()),
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert!(out.starts_with("主 prompt"));
        assert!(out.contains("## 合入闸门失败复检上下文"));
        assert!(out.contains("test_login"));
    }

    #[test]
    fn backtrack_feedback_segment_rendered() {
        let seg = PromptSegments {
            backtrack_feedback: Some("dev blockers：缺少数据流定义".into()),
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert!(out.contains("## 上游回溯反馈"));
        assert!(out.contains("缺少数据流定义"));
    }

    #[test]
    fn retry_feedback_segment_rendered() {
        let seg = PromptSegments {
            retry_feedback: Some("第 1 次：单元测试失败".into()),
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert!(out.contains("## 重试历史摘要"));
    }

    #[test]
    fn blank_optional_segments_are_skipped() {
        let seg = PromptSegments {
            gate_recheck: Some("   ".into()),
            backtrack_feedback: None,
            retry_feedback: Some(String::new()),
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert_eq!(out, "主 prompt");
    }

    #[test]
    fn multiple_segments_keep_fixed_order() {
        let seg = PromptSegments {
            gate_recheck: Some("A".into()),
            backtrack_feedback: Some("B".into()),
            retry_feedback: Some("C".into()),
        };
        let out = build_user_prompt("主", &seg);
        let a = out.find("## 合入闸门失败复检上下文").unwrap();
        let b = out.find("## 上游回溯反馈").unwrap();
        let c = out.find("## 重试历史摘要").unwrap();
        assert!(a < b && b < c);
    }

    // ── prompt_template_hash（决策 137）──

    #[test]
    fn template_hash_is_stable_and_changes_with_content() {
        let a = build_system_prompt("ctx", "persona", "worktree：/wt", &[]);
        let b = build_system_prompt("ctx", "persona", "worktree：/wt", &[]);
        assert_eq!(prompt_template_hash(&a), prompt_template_hash(&b));
        assert_eq!(prompt_template_hash(&a).len(), 16);

        // 用户覆盖 persona → hash 变化（指标可按版本对比）
        let c = build_system_prompt("ctx", "persona 改了", "worktree：/wt", &[]);
        assert_ne!(prompt_template_hash(&a), prompt_template_hash(&c));

        // AGENTS.md 内容不同也改变 hash
        let d = build_system_prompt("ctx 2", "persona", "worktree：/wt", &[]);
        assert_ne!(prompt_template_hash(&a), prompt_template_hash(&d));

        // 工作目录 / 技能清单属于最终组装内容，同样进入 hash（决策 137）
        let e = build_system_prompt("ctx", "persona", "worktree：/other", &[]);
        let f = build_system_prompt("ctx", "persona", "worktree：/wt", &["rtk".to_string()]);
        assert_ne!(prompt_template_hash(&a), prompt_template_hash(&e));
        assert_ne!(prompt_template_hash(&a), prompt_template_hash(&f));
    }

    #[test]
    fn template_hash_is_lowercase_hex() {
        let h = prompt_template_hash("x");
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn persona_placeholder_matches_embedded_defaults() {
        // 内嵌 prompt 与 §10.5 的目录结构一致
        assert_eq!(
            persona_for(Stage::ArchitectDesign, Node::Execute),
            "你是架构设计 agent。"
        );
        let vars = TemplateVars {
            test_command: "cargo test".into(),
            ..Default::default()
        };
        let rendered = render_template(persona_for(Stage::Develop, Node::Execute), &vars);
        assert!(rendered.contains("cargo test"));
    }
}
