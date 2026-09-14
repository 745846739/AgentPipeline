//! 技能（skill）发现与解析（决策 170，修订决策 47）。
//!
//! 决策 47 把 skill 定义为「用户机器上装了对应的外部工具」（PATH 可执行文件名），
//! 只把**名字**列进 system prompt。本模块在保留该语义的前提下扩展出第二类技能：
//! **携带 markdown 正文、正文注入 prompt** 的知识型技能（与 MCP 的分工见 backlog §B.1：
//! skill 是知识/流程指引，MCP 是可调用能力）。
//!
//! 技能的**唯一身份是名字**——两类来源同名即同一个技能，用户文件覆盖内嵌默认：
//!
//! | 来源 | 判定 | 正文 |
//! |---|---|---|
//! | 内嵌默认 | [`EMBEDDED_SKILLS`]（决策 7 的内嵌 persona 先例） | 有 |
//! | 用户 markdown | `{home}/skills/{name}/SKILL.md`（镜像 ZCode 布局，可直接拷贝） | 有，同名覆盖内嵌 |
//! | 外部工具 | PATH 中可执行文件（决策 47 原语义） | 无，只列名字 |
//!
//! 正文的「必须存在且非空」在启动校验（[`crate::config::validate_startup`]）与运行时
//! [`resolve`] 双重把关，口径与 §10.6.4 的 `persona_path` 一致。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// 用户技能目录名（`{home}/skills`）。镜像 ZCode 的 `~/.zcode/skills/{name}/SKILL.md`。
pub const SKILLS_DIR: &str = "skills";

/// 技能文件名（ZCode 布局，可直接把现有 skill 目录拷进来）。
pub const SKILL_FILE: &str = "SKILL.md";

/// 内嵌默认技能（决策 170）：用户目录同名文件可覆盖。
///
/// 这两个是**流水线原生**版本，不是 ZCode 版本的照搬——ZCode 的 `grill-me` 只是
/// 「Call the Skill tool with "grilling"」的调度存根（本系统没有 Skill 工具），而
/// `grilling` 协议的同步多轮对话也无法直接落在本流水线的异步 pending 回路上，
/// 因此改写为经 `submit_metadata.blockers` → `pending(info_insufficient)` → resume
/// 表达提问的版本（决策 79 / 94）。
pub const EMBEDDED_SKILLS: [(&str, &str); 2] =
    [("grilling", GRILLING_BODY), ("to-spec", TO_SPEC_BODY)];

const GRILLING_BODY: &str = r#"# 拷问：把设计树走完再动手

把这次任务的需求当作一棵**设计树**：每个决定都会分叉出挂在它下面的决定。
你的职责是在动手设计之前，把这棵树走到没有悬空的分支。

## 事实自己查，决定问用户

- **事实**（仓库里有什么、用什么测试框架、现有接口长什么样）是你的活：用 `read_file` /
  `list_dir` 去查，**不要**把能在仓库里查到答案的问题甩给用户。
- **决定**（业务口径、取舍、优先级）是用户的：这些才进 blockers。

## 提问方式：一轮一个 frontier

`frontier` 是「当前所有前置决定都已敲定、现在就能问」的那些问题。一轮只问 frontier，
不要问那些答案依赖另一个未决问题的（那是下一轮）。这也符合本节点的机制——你只有一个
出口，一次 pending 就是一轮。

**你唯一的出口是 `submit_metadata`，不能写任何文件**（写 `design.md` 是 execute 的事）：

- frontier 非空 → `readiness: false`，`blockers` 里逐个列出问题，格式：

  `Q1 <问题标题>：<问题正文>。建议：<你的推荐答案与理由>`

  给推荐答案是硬要求——用户是在审批，不是在填空。问题要编号，一次给全 frontier。
- frontier 为空 → `readiness: true`，`blockers: []`，可以进入设计。

## 循环怎么继续

`readiness: false` 会让本节点进入 `pending(info_insufficient)`，用户补充后会**重新回到
本节点**——你会拿到上一轮的问答上下文。此时重新计算 frontier：已敲定的决定会把 frontier
向外推，露出下一批问题。重复，直到 frontier 为空。

## 何时算走完

树上每个分支都访问过、没有静默假设了，就算走完。**不要**在用户确认走完之前开始设计，
也**不要**为了凑轮次把已经能从仓库查到的答案再问一遍。"#;

const TO_SPEC_BODY: &str = r#"# 综合成规格：把已定内容写成 design.md

前置信息已经充分（validate_input 通过），现在**不要再提问**，把已经谈定的内容综合成设计文档。

## 正文骨架

在 §10.3 规定的 design.md 格式基础上，按下面的结构组织（**必需节一个都不能少**，
尤其是「验收标准」——决策 136 要求它是编号清单，下游 test-design / sync-check / review
都机械对照它）：

```
# {task_title}

## 需求概述          （从用户视角讲问题）
## 技术方案          （从用户视角讲解法）
## 用户故事          （编号清单：As a <角色>, I want a <能力>, so that <收益>；尽量覆盖全）
## 实现决策          （模块划分、接口改动、schema、契约、关键交互都在这里）
## 涉及文件          | 文件路径 | 改动类型 | 说明 |      （§10.3 必需）
## 验收标准          - AC-1: ... （§10.3 必需，编号清单，每条可验收，决策 136）
## 测试决策          （测什么、在哪测、参照仓内已有的同类测试）
## 不做的范围        （显式写清 out of scope）
## 风险点            （每条风险配对应措施）
```

**不要**在「实现决策」里写具体文件路径或代码片段——它们很快会过期。唯一例外是某个
原型片段比文字更精确地编码了决定（状态机、reducer、schema、类型形状），那就内联那几行，
并标注它来自原型。

## 产出方式

1. 用 `write_file` 把文档写入 `design.md`（覆盖写入）。
2. 用 `submit_metadata` 提交元数据——字段与 §10.3 一致，一个都不能少：
   `affected_files`、`new_symbols`、`conflict_warnings`、`acceptance_criteria`
   （`[{id, description}]`，与 design.md「验收标准」节**一一对应**）。

## 口径

术语用项目词汇表里的话，涉及架构的地方尊重既有 ADR。判断结论必须给依据，不要只给结论。
不要写执行步骤流水账——那是 develop-design 的活。"#;

/// 技能来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// PATH 中的可执行文件（决策 47 原语义）。只有名字，没有正文。
    Tool,
    /// 内嵌默认正文（[`EMBEDDED_SKILLS`]）。
    Embedded,
    /// 用户 markdown 覆盖（`{home}/skills/{name}/SKILL.md`）。
    Markdown { path: PathBuf },
}

/// 一个可用技能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub source: SkillSource,
}

/// 解析后的技能：正文 `None` 表示工具型技能（只列名字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub name: String,
    pub body: Option<String>,
}

/// 扫描 PATH 得到可执行文件名（决策 47）。
fn path_tool_names() -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    let Some(paths) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    for dir in std::env::split_paths(&paths) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let is_executable = entry.metadata().map(|m| m.is_file()).unwrap_or(false) && {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    entry
                        .metadata()
                        .map(|m| m.permissions().mode() & 0o111 != 0)
                        .unwrap_or(false)
                }
                #[cfg(not(unix))]
                {
                    true
                }
            };
            if is_executable {
                if let Some(name) = entry.file_name().to_str() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names.into_iter().collect()
}

/// `{home}/skills` 下 `{name}/SKILL.md` 的路径。
pub fn skill_file_path(home_root: &Path, name: &str) -> PathBuf {
    home_root.join(SKILLS_DIR).join(name).join(SKILL_FILE)
}

/// 用户技能目录下的技能名（含 `SKILL.md` 的子目录）。按名字排序。
fn markdown_skill_paths(home_root: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let dir = home_root.join(SKILLS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path().join(SKILL_FILE);
        if !path.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        out.insert(name, path);
    }
    out
}

/// 发现全部可用技能（内嵌 ∪ 用户 markdown ∪ PATH 工具），名字去重。
///
/// 用户 markdown 同名覆盖内嵌（来源记为 [`SkillSource::Markdown`]）。
pub fn discover(home_root: &Path) -> Vec<Skill> {
    let markdown = markdown_skill_paths(home_root);

    let mut out: Vec<Skill> = Vec::new();
    for (name, _) in EMBEDDED_SKILLS {
        match markdown.get(name) {
            Some(path) => out.push(Skill {
                name: name.to_string(),
                source: SkillSource::Markdown { path: path.clone() },
            }),
            None => out.push(Skill {
                name: name.to_string(),
                source: SkillSource::Embedded,
            }),
        }
    }

    let embedded: Vec<&str> = EMBEDDED_SKILLS.iter().map(|(n, _)| *n).collect();
    for (name, path) in &markdown {
        if embedded.contains(&name.as_str()) {
            continue; // 已在上面以 Markdown 来源登记
        }
        out.push(Skill {
            name: name.clone(),
            source: SkillSource::Markdown { path: path.clone() },
        });
    }

    let taken: Vec<String> = out.iter().map(|s| s.name.clone()).collect();
    for name in path_tool_names() {
        // 工具型与知识型同名时让位给知识型（有正文的更具体）
        if !taken.contains(&name) {
            out.push(Skill {
                name,
                source: SkillSource::Tool,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 全部可用技能名（启动校验与 `PUT /stage-configs` 用）。
pub fn skill_names(home_root: &Path) -> Vec<String> {
    discover(home_root).into_iter().map(|s| s.name).collect()
}

/// 剥离 YAML frontmatter（`---` 包围块），只做文本处理，不引 YAML 依赖。
///
/// frontmatter 必须在文件开头，并以单独一行的 `---` 收尾；不构成 frontmatter 时原样返回。
fn strip_frontmatter(raw: &str) -> String {
    let trimmed = raw.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return trimmed.to_string();
    };
    let Some(end) = rest
        .lines()
        .position(|l| l.trim_end() == "---" || l.trim_end() == "...")
    else {
        return trimmed.to_string();
    };
    rest.lines()
        .skip(end + 1)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start()
        .to_string()
}

/// 读取一个知识型技能的正文（内嵌或用户文件）。
///
/// `name` 必须存在于 [`discover`] 的结果中。工具型技能返回 `Ok(None)`。
fn body_of(home_root: &Path, name: &str) -> Result<Option<String>> {
    let path = skill_file_path(home_root, name);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let body = strip_frontmatter(&raw).trim().to_string();
            if body.is_empty() {
                return Err(Error::Config(format!(
                    "技能 {name} 的正文为空：{}",
                    path.display()
                )));
            }
            Ok(Some(body))
        }
        Err(_) => match EMBEDDED_SKILLS.iter().find(|(n, _)| *n == name) {
            Some((_, body)) => Ok(Some(body.trim().to_string())),
            None => Ok(None), // 工具型技能：只有名字
        },
    }
}

/// 解析声明的一组技能为「名字 + 可选正文」，保持声明顺序、去重。
///
/// 正文为空（用户文件写了空内容）→ [`Error::Config`]，与 `persona_path` 同口径。
pub fn resolve(home_root: &Path, declared: &[String]) -> Result<Vec<ResolvedSkill>> {
    let mut out: Vec<ResolvedSkill> = Vec::new();
    for name in declared {
        if out.iter().any(|s| &s.name == name) {
            continue;
        }
        out.push(ResolvedSkill {
            name: name.clone(),
            body: body_of(home_root, name)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_skill(root: &Path, name: &str, content: &str) {
        let dir = root.join(SKILLS_DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), content).unwrap();
    }

    #[test]
    fn embedded_skills_are_discovered_without_any_home_files() {
        let home = tmp();
        let names = skill_names(home.path());
        assert!(names.contains(&"grilling".to_string()));
        assert!(names.contains(&"to-spec".to_string()));
    }

    #[test]
    fn embedded_skills_carry_bodies() {
        let home = tmp();
        let resolved = resolve(home.path(), &["grilling".into(), "to-spec".into()]).unwrap();
        assert_eq!(resolved.len(), 2);
        // 正文是流水线原生版本：grilling 讲 pending 回路，to-spec 守决策 136
        let grilling = resolved[0].body.as_deref().unwrap();
        assert!(grilling.contains("info_insufficient"), "{grilling}");
        assert!(grilling.contains("frontier"), "{grilling}");
        let to_spec = resolved[1].body.as_deref().unwrap();
        assert!(to_spec.contains("验收标准"), "{to_spec}");
        assert!(to_spec.contains("acceptance_criteria"), "{to_spec}");
    }

    #[test]
    fn user_markdown_overrides_embedded_body() {
        let home = tmp();
        write_skill(home.path(), "grilling", "用户自己的拷问流程");
        let resolved = resolve(home.path(), &["grilling".into()]).unwrap();
        assert_eq!(resolved[0].body.as_deref(), Some("用户自己的拷问流程"));
        // 来源登记为 Markdown 覆盖
        let found = discover(home.path());
        let s = found.iter().find(|s| s.name == "grilling").unwrap();
        assert!(matches!(s.source, SkillSource::Markdown { .. }), "{s:?}");
    }

    #[test]
    fn frontmatter_is_stripped_from_user_file() {
        let home = tmp();
        write_skill(
            home.path(),
            "grilling",
            "---\nname: grill-me\ndescription: x\n---\n\n正文从这里开始",
        );
        let resolved = resolve(home.path(), &["grilling".into()]).unwrap();
        assert_eq!(resolved[0].body.as_deref(), Some("正文从这里开始"));
    }

    #[test]
    fn empty_user_file_is_config_error() {
        let home = tmp();
        write_skill(home.path(), "grilling", "   \n");
        let err = resolve(home.path(), &["grilling".into()]).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        assert!(err.to_string().contains("正文为空"), "{err}");
    }

    #[test]
    fn tool_skill_has_no_body() {
        let home = tmp();
        // 工具型技能（PATH 可执行文件）不在 HOME 里，正文为 None——用未声明的名字验证
        let resolved = resolve(home.path(), &["definitely-not-a-knowledge-skill".into()]).unwrap();
        assert!(resolved[0].body.is_none());
    }

    #[test]
    fn unknown_markdown_dir_without_skill_file_is_ignored() {
        let home = tmp();
        std::fs::create_dir_all(home.path().join(SKILLS_DIR).join("not-a-skill")).unwrap();
        assert!(!skill_names(home.path()).contains(&"not-a-skill".to_string()));
    }

    #[test]
    fn resolve_dedups_and_preserves_declaration_order() {
        let home = tmp();
        let resolved = resolve(
            home.path(),
            &["to-spec".into(), "grilling".into(), "to-spec".into()],
        )
        .unwrap();
        assert_eq!(
            resolved.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["to-spec", "grilling"]
        );
    }

    #[test]
    fn skill_file_path_matches_zcode_layout() {
        let p = skill_file_path(Path::new("/home/u/.agentpipeline"), "grilling");
        assert!(p.ends_with("skills/grilling/SKILL.md"), "{}", p.display());
    }
}
