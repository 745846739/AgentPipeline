//! System / user prompt 组装（决策 51 / 28 / 7 / 109 / 126 / 138 / 31 / 137）。
//!
//! 段落顺序固定为 `[基线前言][AGENTS.md][persona][格式规则]`——固定前缀保证 prompt cache
//! 稳定命中（§12.13.5）。user prompt 的追加段（gate_recheck / backtrack-feedback /
//! retry-feedback）**首轮为空不渲染**。

use std::path::{Path, PathBuf};

use crate::agent::bounded_read::{self, Offloaded};
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
///
/// **读走阻塞池**（决策 302）：这个读点正是 2026-09-27 那次 4 小时挂死的那一处
/// （完全磁盘访问弹窗无人应答 → `open()` 挂在同步系统调用里 → 占死一个 worker）。
/// 「读不到」与「读超界」在这里是**同一条降级路**——都是「拿不到项目上下文」，
/// 差别只在可观测性（超界会进 `bounded_read::stats()` 与日志）。
pub async fn load_agents_context(
    project_root: &Path,
    language: Option<&str>,
    test_framework: Option<&str>,
) -> String {
    let fallback = || default_agents_context(project_root, language, test_framework);
    match bounded_read::read_to_string("agents_md", &project_root.join("AGENTS.md")).await {
        Offloaded::Done(Ok(content)) if !content.trim().is_empty() => {
            format!("## 项目上下文（AGENTS.md）\n{}", content.trim())
        }
        _ => fallback(),
    }
}

/// 组装 system prompt：
/// `[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能清单][格式规则]`。
/// 固定前缀保证 prompt cache 稳定命中（§12.13.5）；worktree / 任务目录是任务级
/// 常量，不破坏同一任务内重试的缓存。
///
/// 技能段（决策 170 / 172）分三态渲染：
/// - **全文态**：`### {name}` + 正文，正文进 prompt 因此 `prompt_template_hash` 对其敏感
///   （决策 137）；
/// - **名字态**：`- {name}`，正文由 `Skill` 工具按需拉取（票 06），**不进** prompt，
///   故 hash 对它的正文变化钝感（票 05 的显式要求）；
/// - **目录态**：`- {name}: {description}`，渐进披露——让模型知道有哪些能力可用，
///   而不必预载全部正文。
pub fn build_system_prompt(
    agents_context: &str,
    persona: &str,
    workdirs: &str,
    skills: &[crate::agent::skills::ResolvedSkill],
) -> String {
    use crate::agent::skills::SkillRender;

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
            match &s.render {
                SkillRender::Full { body } if !body.trim().is_empty() => {
                    out.push_str(&format!("### {}\n{}\n", s.name, body.trim()));
                }
                SkillRender::Full { .. } => out.push_str(&format!("- {}\n", s.name)),
                SkillRender::Name => out.push_str(&format!("- {}\n", s.name)),
                SkillRender::Catalogue { description } => match description
                    .as_deref()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                {
                    Some(d) => out.push_str(&format!("- {}: {d}\n", s.name)),
                    None => out.push_str(&format!("- {}\n", s.name)),
                },
            }
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
///
/// **读走阻塞池**（决策 302）：persona 覆盖目录同样可能落在受保护路径上
/// （`[prompts] dir` 指到 `~/Documents/...` 时），只包住 AGENTS.md 就是打地鼠。
/// 读不到 / 读超界都回落内嵌默认——与「覆盖文件不存在」同一条路。
pub async fn resolve_persona(
    prompts_dir: &Path,
    stage: Stage,
    node: Node,
    embedded_default: &str,
) -> ResolvedPersona {
    let path = prompts_dir
        .join(stage.prompt_dir())
        .join(format!("{}.md", node.as_str()));
    match bounded_read::read_to_string("persona", &path).await {
        Offloaded::Done(Ok(content)) if !content.trim().is_empty() => ResolvedPersona {
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

/// user prompt 的可选追加段（决策 79 / 109 / 126 / 133 / 138）。
///
/// 约定：**首轮为空不渲染**——空段不产生额外的标题与空行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSegments {
    /// 闸门复检上下文（决策 109）：`gate_recheck = true` 时非空。
    pub gate_recheck: Option<String>,
    /// sync-check backtrack 的双方 blockers（决策 126）。
    pub backtrack_feedback: Option<String>,
    /// `info_insufficient` 的用户补充输入（决策 79）。
    pub user_input: Option<String>,
    /// review 打回后 develop 重入必须修改项（决策 133）。
    pub review_required_changes: Option<String>,
    /// develop / test 重试耗尽的失败摘要（决策 138）。
    pub retry_feedback: Option<String>,
    /// develop 重入的零提交事实与落提交指令（决策 391）：`develop_code_gate` 的确定性
    /// 守卫判定分支自有提交数为 0 时落盘，重入 develop.execute 时注入。**只有这一态非空**。
    pub zero_commit: Option<String>,
    /// 超时梯子第 3 档（空白重跑）的起跑简报（决策 376 裁决② · 票 04）：任务描述 +
    /// 阶段产物文件清单 + 未提交改动清单 + 最近收口摘要。**只有这一档非空**——它替代
    /// 全卷转录，让「上一轮的侦察」不必每次重置都重新买一遍。
    pub continuation_brief: Option<String>,
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
        ("## 用户补充输入", segments.user_input.as_deref()),
        (
            "## 评审必须修改项",
            segments.review_required_changes.as_deref(),
        ),
        ("## 重试历史摘要", segments.retry_feedback.as_deref()),
        ("## 零提交事实与落提交指令", segments.zero_commit.as_deref()),
        (
            "## 续接简报（超时空白重跑，不带全卷转录）",
            segments.continuation_brief.as_deref(),
        ),
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

    /// 工具型技能（只有名字，无正文；决策 47）。
    fn tool_skill(name: &str) -> crate::agent::skills::ResolvedSkill {
        crate::agent::skills::ResolvedSkill::name_only(name)
    }

    /// 知识型技能（全文注入；决策 170 / 172）。
    fn knowledge_skill(name: &str, body: &str) -> crate::agent::skills::ResolvedSkill {
        crate::agent::skills::ResolvedSkill::full(name, body)
    }

    /// 目录态技能（渐进披露：名字 + 描述；决策 172④ / 票 05）。
    fn catalogue_skill(name: &str, desc: Option<&str>) -> crate::agent::skills::ResolvedSkill {
        crate::agent::skills::ResolvedSkill::catalogue(name, desc.map(str::to_string))
    }

    // ── 组装顺序 golden ──

    #[test]
    fn system_prompt_section_order_is_golden() {
        let prompt = build_system_prompt(
            "## 项目上下文\n仓库 X",
            "PERSONA_BODY",
            "worktree：/wt\n任务目录：/td",
            &[tool_skill("rtk")],
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

    // ── 技能正文注入（决策 170）──

    #[test]
    fn tool_skill_renders_as_bare_bullet() {
        // 决策 47 原样：工具型技能仍只是一行 `- name`
        let prompt = build_system_prompt("ctx", "persona", "worktree：/wt", &[tool_skill("rtk")]);
        assert!(prompt.contains("## 已启用技能\n- rtk\n"), "{prompt}");
        assert!(!prompt.contains("### rtk"), "{prompt}");
    }

    #[test]
    fn knowledge_skill_body_is_injected() {
        let prompt = build_system_prompt(
            "ctx",
            "persona",
            "worktree：/wt",
            &[knowledge_skill("grilling", "把设计树走完再动手")],
        );
        assert!(prompt.contains("## 已启用技能"), "{prompt}");
        assert!(prompt.contains("### grilling"), "{prompt}");
        assert!(prompt.contains("把设计树走完再动手"), "{prompt}");
    }

    #[test]
    fn mixed_skills_render_both_forms_in_declaration_order() {
        let prompt = build_system_prompt(
            "ctx",
            "persona",
            "worktree：/wt",
            &[tool_skill("rtk"), knowledge_skill("to-spec", "综合成规格")],
        );
        let bullet = prompt.find("- rtk").unwrap();
        let heading = prompt.find("### to-spec").unwrap();
        assert!(bullet < heading, "保持声明顺序：{prompt}");
    }

    #[test]
    fn skill_body_changes_prompt_hash() {
        // 正文进 prompt，故 hash 对正文敏感（决策 137）
        let a = build_system_prompt("ctx", "p", "wt", &[tool_skill("grilling")]);
        let b = build_system_prompt("ctx", "p", "wt", &[knowledge_skill("grilling", "正文")]);
        assert_ne!(prompt_template_hash(&a), prompt_template_hash(&b));
    }

    // ── 票 05（决策 172④）：三态渲染 ──

    /// 目录态渲染为 `- {name}: {description}`——名字 + 描述，**不含正文**。
    #[test]
    fn catalogue_skill_renders_name_and_description() {
        let prompt = build_system_prompt(
            "ctx",
            "persona",
            "worktree：/wt",
            &[catalogue_skill("research", Some("调研一个主题"))],
        );
        assert!(
            prompt.contains("## 已启用技能\n- research: 调研一个主题\n"),
            "{prompt}"
        );
        assert!(!prompt.contains("### research"), "{prompt}");
    }

    /// 目录态无 `description` 时退化为普通子弹（不渲染空冒号）。
    #[test]
    fn catalogue_skill_without_description_is_bare_bullet() {
        let prompt = build_system_prompt(
            "ctx",
            "persona",
            "worktree：/wt",
            &[catalogue_skill("mystery", None)],
        );
        assert!(prompt.contains("- mystery\n"), "{prompt}");
        assert!(!prompt.contains("mystery:"), "{prompt}");
    }

    /// 票 05 的 hash 要求：`prompt_template_hash` **只对全文态敏感**。
    ///
    /// 从真实技能根解析（经 [`crate::agent::skills::resolve`]）才有意义——名字态若在解析
    /// 时顺手读了正文，这条就会红。同一个技能换一份正文：
    /// 名字态 hash 不变（正文不进 prompt），全文态 hash 变（正文进 prompt）。
    #[test]
    fn hash_is_sensitive_to_body_only_in_full_mode() {
        use crate::agent::skills::{resolve, SkillDecl, SkillMode};

        let render = |mode: SkillMode, body: &str| {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join("s");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), body).unwrap();
            let decl = SkillDecl {
                name: "s".into(),
                mode,
                trusted: true,
            };
            let skills = resolve(root.path(), &[decl]).unwrap();
            build_system_prompt("ctx", "p", "wt", &skills)
        };

        // 名字态：正文不进 prompt，换正文 hash 不变
        let name_a = render(SkillMode::Name, "正文甲");
        let name_b = render(SkillMode::Name, "正文乙");
        assert!(!name_a.contains("正文甲"), "{name_a}");
        assert_eq!(
            prompt_template_hash(&name_a),
            prompt_template_hash(&name_b),
            "名字态的正文变化不得造成 hash 抖动（票 05）"
        );

        // 全文态：正文进 prompt，换正文 hash 必变
        let full_a = render(SkillMode::Full, "正文甲");
        let full_b = render(SkillMode::Full, "正文乙");
        assert!(full_a.contains("正文甲"), "{full_a}");
        assert_ne!(
            prompt_template_hash(&full_a),
            prompt_template_hash(&full_b),
            "全文态对正文必须敏感（决策 137）"
        );
    }

    /// 目录态不含正文，故目录项的描述变化不影响正文级敏感度（构造上就不带正文）。
    #[test]
    fn catalogue_state_never_carries_body() {
        let prompt = build_system_prompt(
            "ctx",
            "p",
            "wt",
            &[
                catalogue_skill("s", Some("描述")),
                catalogue_skill("t", None),
            ],
        );
        assert!(prompt.contains("- s: 描述"), "{prompt}");
        assert!(prompt.contains("- t\n"), "{prompt}");
        assert!(!prompt.contains("### "), "目录态不得有正文标题：{prompt}");
    }

    /// 三态并存时的段落顺序：声明的（全文 / 名字）在前，目录项在后。
    #[test]
    fn catalogue_after_declared_skills() {
        let prompt = build_system_prompt(
            "ctx",
            "persona",
            "worktree：/wt",
            &[
                knowledge_skill("enabled", "已启用正文"),
                catalogue_skill("available", Some("可选")),
            ],
        );
        let heading = prompt.find("### enabled").unwrap();
        let bullet = prompt.find("- available: 可选").unwrap();
        assert!(heading < bullet, "声明的技能应排在目录之前：{prompt}");
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

    #[tokio::test]
    async fn agents_md_content_becomes_agents_context() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("AGENTS.md"),
            "# 项目约定\n- 用 just test 跑测试",
        )
        .unwrap();
        let ctx = load_agents_context(tmp.path(), Some("Rust"), Some("cargo")).await;
        assert!(ctx.starts_with("## 项目上下文（AGENTS.md）"));
        assert!(ctx.contains("# 项目约定"));
        assert!(ctx.contains("just test"));
    }

    #[tokio::test]
    async fn missing_or_blank_agents_md_falls_back_to_default() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = load_agents_context(tmp.path(), Some("Rust"), None).await;
        assert!(ctx.contains("本仓库无 AGENTS.md"));

        std::fs::write(tmp.path().join("AGENTS.md"), "   \n").unwrap();
        assert!(load_agents_context(tmp.path(), None, None)
            .await
            .contains("本仓库无 AGENTS.md"));
    }

    // ── prompts/ 覆盖生效 ──

    #[tokio::test]
    async fn prompts_dir_override_wins_over_embedded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("architect_design");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("execute.md"), "覆盖后的 persona").unwrap();

        let got = resolve_persona(
            tmp.path(),
            Stage::ArchitectDesign,
            Node::Execute,
            "内嵌 persona",
        )
        .await;
        assert!(got.from_override);
        assert_eq!(got.content, "覆盖后的 persona");
        assert!(got.path.unwrap().ends_with("architect_design/execute.md"));
    }

    #[test]
    fn prompts_root_prefers_config_dir_and_falls_back_to_home() {
        // 票 16：`[prompts] dir` 覆盖目录优先；未配置回落 {home}/prompts
        let home_prompts = Path::new("/home/u/.agentpipeline/prompts");
        assert_eq!(
            prompts_root(home_prompts, Some(Path::new("/custom/prompts"))),
            PathBuf::from("/custom/prompts")
        );
        assert_eq!(prompts_root(home_prompts, None), home_prompts);
    }

    #[tokio::test]
    async fn overridden_prompts_root_is_what_resolve_persona_reads() {
        // 覆盖目录中存在 persona 时，resolve_persona 必须读到它而不是内嵌默认
        let home = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let dir = custom.path().join("develop");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("execute.md"), "自定义目录 persona").unwrap();

        let root = prompts_root(&home.path().join("prompts"), Some(custom.path()));
        let got = resolve_persona(&root, Stage::Develop, Node::Execute, "内嵌 persona").await;
        assert!(got.from_override);
        assert_eq!(got.content, "自定义目录 persona");

        // 覆盖目录里没有该文件 → 回落内嵌默认（不因目录缺失而报错）
        let empty = tempfile::tempdir().unwrap();
        let root = prompts_root(&home.path().join("prompts"), Some(empty.path()));
        let got = resolve_persona(&root, Stage::Develop, Node::Execute, "内嵌 persona").await;
        assert!(!got.from_override);
        assert_eq!(got.content, "内嵌 persona");
    }

    #[tokio::test]
    async fn embedded_persona_used_when_no_override() {
        let tmp = tempfile::tempdir().unwrap();
        let got = resolve_persona(tmp.path(), Stage::Develop, Node::Execute, "内嵌 persona").await;
        assert!(!got.from_override);
        assert_eq!(got.content, "内嵌 persona");
        assert!(got.path.is_none());
    }

    #[tokio::test]
    async fn empty_override_file_falls_back_to_embedded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("develop");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("execute.md"), "   \n").unwrap();
        let got = resolve_persona(tmp.path(), Stage::Develop, Node::Execute, "内嵌 persona").await;
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
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert_eq!(out, "主 prompt");
    }

    #[test]
    fn multiple_segments_keep_fixed_order() {
        let seg = PromptSegments {
            gate_recheck: Some("A".into()),
            backtrack_feedback: Some("B".into()),
            review_required_changes: Some("D".into()),
            user_input: Some("E".into()),
            retry_feedback: Some("C".into()),
            zero_commit: Some("G".into()),
            continuation_brief: Some("F".into()),
        };
        let out = build_user_prompt("主", &seg);
        let a = out.find("## 合入闸门失败复检上下文").unwrap();
        let b = out.find("## 上游回溯反馈").unwrap();
        let e = out.find("## 用户补充输入").unwrap();
        let d = out.find("## 评审必须修改项").unwrap();
        let c = out.find("## 重试历史摘要").unwrap();
        let g = out.find("## 零提交事实与落提交指令").unwrap();
        let f = out
            .find("## 续接简报（超时空白重跑，不带全卷转录）")
            .unwrap();
        assert!(a < b && b < e && e < d && d < c && c < g && g < f);
    }

    /// 票 04：空白重跑的简报段被渲染，且是**独立一段**（不是塞进别的段里）。
    #[test]
    fn continuation_brief_segment_rendered() {
        let seg = PromptSegments {
            continuation_brief: Some("## 任务描述\n### 走查\n把清单落盘".into()),
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert!(out.contains("## 续接简报（超时空白重跑，不带全卷转录）"));
        assert!(out.contains("把清单落盘"));
    }

    #[test]
    fn review_required_changes_segment_rendered() {
        let seg = PromptSegments {
            review_required_changes: Some("修改 `src/lib.rs`".into()),
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert!(out.contains("## 评审必须修改项"));
        assert!(out.contains("src/lib.rs"));
    }

    #[test]
    fn user_input_segment_rendered() {
        let seg = PromptSegments {
            user_input: Some("部署环境是生产 k8s".into()),
            ..Default::default()
        };
        let out = build_user_prompt("主 prompt", &seg);
        assert!(out.contains("## 用户补充输入"));
        assert!(out.contains("生产 k8s"));
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
        let f = build_system_prompt("ctx", "persona", "worktree：/wt", &[tool_skill("rtk")]);
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
