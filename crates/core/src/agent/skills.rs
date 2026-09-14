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
//! | 用户 markdown | `{skills_root}/{name}/SKILL.md`（镜像 ZCode 布局，可直接拷贝） | 有，同名覆盖内嵌 |
//! | 外部工具 | PATH 中可执行文件（决策 47 原语义） | 无，只列名字 |
//!
//! **技能根唯一**（决策 172，修订决策 47）：默认 `{home}/skills`，可由 `[skills] dir`
//! 覆盖（如指到 `~/.zcode/skills`）——本模块的每个入口都接收**技能根**本身，不再自己
//! 拼 `skills` 目录名；覆盖解析见 [`crate::config::SkillsConfig::resolved_dir`]。
//!
//! 正文的「必须存在且非空」在启动校验（[`crate::config::validate_startup`]）与运行时
//! [`resolve`] 双重把关，口径与 §10.6.4 的 `persona_path` 一致；frontmatter 的 `name`
//! 与目录名不一致同样在启动时 fail fast（[`validate_names`]）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// 默认技能目录名（`{home}/skills`）。镜像 ZCode 的 `~/.zcode/skills/{name}/SKILL.md`。
///
/// 技能根本身是**唯一入口**（决策 172）：默认由它拼出，`[skills] dir` 配置时整体替换。
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
    /// 用户 markdown 覆盖（`{skills_root}/{name}/SKILL.md`）。
    Markdown { path: PathBuf },
}

/// frontmatter 的四个键（决策 172②：解析但不做语义检查）。
///
/// 逐行 `key: value` 轻量解析，**不引 YAML 依赖**；缺失 / 畸形一律按缺省处理，
/// 只有 `name` 与目录名不符才是 fail fast（那也是 [`Skill`] 的构造条件，不在此处）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillFrontmatter {
    /// `description`——进技能目录（渐进披露只给名字 + 描述）。
    pub description: Option<String>,
    /// `disable-model-invocation: true`——不被自动注入（选型 D）。
    pub disable_model_invocation: bool,
    /// `license`——**只解析不生效**（决策 172：无工具权限授予层）。
    pub license: Option<String>,
    /// `allowed-tools`——同上，只解析不生效（规范标记实验性）。
    pub allowed_tools: Option<String>,
    /// frontmatter 里显式写的 `name`（规范要求与父目录同名，见 [`validate_names`]）。
    pub name: Option<String>,
}

/// 一个可用技能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub source: SkillSource,
    /// frontmatter 解析结果（工具型技能为空缺省）。
    pub frontmatter: SkillFrontmatter,
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

/// 技能根下 `{name}/SKILL.md` 的路径。
///
/// `skills_root` 就是技能根**本身**（默认 `{home}/skills`，可由 `[skills] dir` 覆盖），
/// 不含 `skills` 目录名（决策 172）。
pub fn skill_file_path(skills_root: &Path, name: &str) -> PathBuf {
    skills_root.join(name).join(SKILL_FILE)
}

/// 技能根下的技能名（含 `SKILL.md` 的子目录）。按名字排序。
fn markdown_skill_paths(skills_root: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(skills_root) else {
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

/// 发现全部可用技能（内嵌 ∪ 技能根 markdown ∪ PATH 工具），名字去重。
///
/// 技能根下的同名文件覆盖内嵌（来源记为 [`SkillSource::Markdown`]）。
pub fn discover(skills_root: &Path) -> Vec<Skill> {
    let markdown = markdown_skill_paths(skills_root);

    let mut out: Vec<Skill> = Vec::new();
    for (name, _) in EMBEDDED_SKILLS {
        match markdown.get(name) {
            Some(path) => out.push(Skill {
                name: name.to_string(),
                source: SkillSource::Markdown { path: path.clone() },
                frontmatter: read_frontmatter(path),
            }),
            None => out.push(Skill {
                name: name.to_string(),
                source: SkillSource::Embedded,
                frontmatter: SkillFrontmatter::default(),
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
            frontmatter: read_frontmatter(path),
        });
    }

    let taken: Vec<String> = out.iter().map(|s| s.name.clone()).collect();
    for name in path_tool_names() {
        // 工具型与知识型同名时让位给知识型（有正文的更具体）
        if !taken.contains(&name) {
            out.push(Skill {
                name,
                source: SkillSource::Tool,
                frontmatter: SkillFrontmatter::default(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 全部可用技能名（启动校验与 `PUT /stage-configs` 用）。
pub fn skill_names(skills_root: &Path) -> Vec<String> {
    discover(skills_root).into_iter().map(|s| s.name).collect()
}

/// 读取并解析一个技能文件的 frontmatter（读不到 → 全缺省，不报错）。
fn read_frontmatter(path: &Path) -> SkillFrontmatter {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse_frontmatter(&raw).0,
        Err(_) => SkillFrontmatter::default(),
    }
}

/// 拆出 frontmatter 块与正文。
///
/// frontmatter 必须在文件开头，并以单独一行的 `---`（或 `...`）收尾；不构成 frontmatter
/// 时返回 `(None, raw.trim_start())`。**未闭合按无 frontmatter 处理**——不 fail fast，
/// 正文照读（决策 172②：加载器不因内容拒绝合法 markdown）。
///
/// 正文经 `lines()` 重组，因此行尾统一为 `\n`（与既有剥离行为逐字相同——正文进
/// system prompt，行尾差异会改 `prompt_template_hash`，决策 137）。
///
/// 第二个返回值是「开头 `---` 是否为独立一行」——正文剥离沿用历史的宽松口径（只看
/// 前缀），但**键的解释与 `name` 校验只认规范的 frontmatter 块**，否则一段以水平线
/// `---` 开头的正文会被误读成 frontmatter，把无害的 Markdown 升级成启动失败。
fn split_frontmatter(raw: &str) -> (Option<String>, String, bool) {
    let trimmed = raw.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (None, trimmed.to_string(), false);
    };
    // 规范的 frontmatter：开头 `---` 自成一行（`---\n` / `---\r\n` / 整个文件就是 `---`）
    let well_formed = rest.is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n");
    let lines: Vec<&str> = rest.lines().collect();
    let Some(end) = lines
        .iter()
        .position(|l| l.trim_end() == "---" || l.trim_end() == "...")
    else {
        return (None, trimmed.to_string(), false);
    };
    let block = lines[..end].join("\n");
    let body = lines[end + 1..].join("\n").trim_start().to_string();
    (Some(block), body, well_formed)
}

/// 解析 frontmatter 四键（决策 172②）：逐行 `key: value`，无 YAML 依赖。
///
/// 值只做最朴素的 trim 与去引号；布尔只认 `true` / `false`，其余按缺省。
/// 返回（解析结果，正文）。
pub fn parse_frontmatter(raw: &str) -> (SkillFrontmatter, String) {
    let (block, body, well_formed) = split_frontmatter(raw);
    let Some(block) = block.filter(|_| well_formed) else {
        return (SkillFrontmatter::default(), body);
    };
    let mut fm = SkillFrontmatter::default();
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote(value.trim());
        match key {
            "description" => fm.description = Some(value),
            "disable-model-invocation" => {
                fm.disable_model_invocation = value == "true";
            }
            "license" => fm.license = Some(value),
            "allowed-tools" => fm.allowed_tools = Some(value),
            "name" => fm.name = Some(value),
            _ => {} // 其余键（如 argument-hint）忽略，不 fail fast
        }
    }
    (fm, body)
}

/// 去掉值两端成对的引号（`"x"` / `'x'` → `x`）。
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// 校验技能根下每个技能的 frontmatter `name`（写了就必须与目录名一致）。
///
/// 对齐 Agent Skills 规范「name 必须与父目录同名」；名字是唯一身份，这条不变量不能被
/// frontmatter 悄悄覆盖（决策 172）。不一致 → [`Error::Config`]，供启动校验调用。
pub fn validate_names(skills_root: &Path) -> Result<()> {
    for (name, path) in markdown_skill_paths(skills_root) {
        let fm = read_frontmatter(&path);
        if let Some(declared) = fm.name.as_deref() {
            if !declared.is_empty() && declared != name {
                return Err(Error::Config(format!(
                    "技能 {name} 的 frontmatter name 与目录名不一致：{declared}（\
                     名字是唯一身份，须与父目录同名）"
                )));
            }
        }
    }
    Ok(())
}

/// 读取一个知识型技能的正文（内嵌或技能根下的用户文件）。
///
/// `name` 必须存在于 [`discover`] 的结果中。工具型技能返回 `Ok(None)`。
fn body_of(skills_root: &Path, name: &str) -> Result<Option<String>> {
    let path = skill_file_path(skills_root, name);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let body = parse_frontmatter(&raw).1.trim().to_string();
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
/// `skills_root` 是技能根本身（默认 `{home}/skills`，`[skills] dir` 可覆盖）。
/// 正文为空（用户文件写了空内容）→ [`Error::Config`]，与 `persona_path` 同口径。
pub fn resolve(skills_root: &Path, declared: &[String]) -> Result<Vec<ResolvedSkill>> {
    let mut out: Vec<ResolvedSkill> = Vec::new();
    for name in declared {
        if out.iter().any(|s| &s.name == name) {
            continue;
        }
        out.push(ResolvedSkill {
            name: name.clone(),
            body: body_of(skills_root, name)?,
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

    /// 默认技能根（`{home}/skills`）——与 `Home::skills_dir` 同构，避免测试里散落拼路径。
    fn root_of(home: &Path) -> PathBuf {
        home.join(SKILLS_DIR)
    }

    fn write_skill(root: &Path, name: &str, content: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), content).unwrap();
    }

    #[test]
    fn embedded_skills_are_discovered_without_any_home_files() {
        let home = tmp();
        let names = skill_names(&root_of(home.path()));
        assert!(names.contains(&"grilling".to_string()));
        assert!(names.contains(&"to-spec".to_string()));
    }

    #[test]
    fn embedded_skills_carry_bodies() {
        let home = tmp();
        let resolved = resolve(
            &root_of(home.path()),
            &["grilling".into(), "to-spec".into()],
        )
        .unwrap();
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
        write_skill(&root_of(home.path()), "grilling", "用户自己的拷问流程");
        let resolved = resolve(&root_of(home.path()), &["grilling".into()]).unwrap();
        assert_eq!(resolved[0].body.as_deref(), Some("用户自己的拷问流程"));
        // 来源登记为 Markdown 覆盖
        let found = discover(&root_of(home.path()));
        let s = found.iter().find(|s| s.name == "grilling").unwrap();
        assert!(matches!(s.source, SkillSource::Markdown { .. }), "{s:?}");
    }

    #[test]
    fn frontmatter_is_stripped_from_user_file() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "grilling",
            "---\nname: grilling\ndescription: x\n---\n\n正文从这里开始",
        );
        let resolved = resolve(&root_of(home.path()), &["grilling".into()]).unwrap();
        assert_eq!(resolved[0].body.as_deref(), Some("正文从这里开始"));
    }

    #[test]
    fn empty_user_file_is_config_error() {
        let home = tmp();
        write_skill(&root_of(home.path()), "grilling", "   \n");
        let err = resolve(&root_of(home.path()), &["grilling".into()]).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        assert!(err.to_string().contains("正文为空"), "{err}");
    }

    /// CRLF 文件的正文行尾统一为 `\n`——与既有剥离行为逐字相同。
    ///
    /// 正文进 system prompt，行尾差异会改 `prompt_template_hash`（决策 137），
    /// 因此这条不是洁癖而是兼容性要求（票 01：未配置时行为零变化）。
    #[test]
    fn crlf_body_line_endings_are_normalized() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "win-skill",
            "---\r\nname: win-skill\r\ndescription: x\r\n---\r\n\r\n第一行\r\n第二行\r\n",
        );
        let resolved = resolve(&root_of(home.path()), &["win-skill".into()]).unwrap();
        let body = resolved[0].body.as_deref().unwrap();
        assert_eq!(body, "第一行\n第二行", "行尾应统一为 \\n：{body:?}");
        assert!(!body.contains('\r'), "{body:?}");
    }

    #[test]
    fn tool_skill_has_no_body() {
        let home = tmp();
        // 工具型技能（PATH 可执行文件）不在技能根里，正文为 None——用未声明的名字验证
        let resolved = resolve(
            &root_of(home.path()),
            &["definitely-not-a-knowledge-skill".into()],
        )
        .unwrap();
        assert!(resolved[0].body.is_none());
    }

    #[test]
    fn unknown_markdown_dir_without_skill_file_is_ignored() {
        let home = tmp();
        std::fs::create_dir_all(root_of(home.path()).join("not-a-skill")).unwrap();
        assert!(!skill_names(&root_of(home.path())).contains(&"not-a-skill".to_string()));
    }

    #[test]
    fn resolve_dedups_and_preserves_declaration_order() {
        let home = tmp();
        let resolved = resolve(
            &root_of(home.path()),
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
        // 技能根就是 `{home}/skills` 时（生产默认），布局与 ZCode 一致
        let p = skill_file_path(
            &Path::new("/home/u/.agentpipeline").join(SKILLS_DIR),
            "grilling",
        );
        assert!(p.ends_with("skills/grilling/SKILL.md"), "{}", p.display());
    }

    /// 票 01：技能根本身可直接落在任意目录（`[skills] dir` 覆盖后就是这种形态）。
    #[test]
    fn discovery_uses_skills_root_verbatim() {
        let external = tmp();
        write_skill(external.path(), "my-skill", "外部技能正文");
        // 技能根 = 外部目录本身，不是 `{外部目录}/skills`
        let resolved = resolve(external.path(), &["my-skill".into()]).unwrap();
        assert_eq!(resolved[0].body.as_deref(), Some("外部技能正文"));
        assert!(skill_names(external.path()).contains(&"my-skill".to_string()));
        assert!(validate_names(external.path()).is_ok());
    }

    // ── 票 02：frontmatter 四键解析 ──

    #[test]
    fn frontmatter_reads_four_keys() {
        let raw = "---\nname: s\ndescription: 描述文本\ndisable-model-invocation: true\n\
                   license: Apache-2.0\nallowed-tools: read_file, list_dir\n---\n\n正文";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(fm.name.as_deref(), Some("s"));
        assert_eq!(fm.description.as_deref(), Some("描述文本"));
        assert!(fm.disable_model_invocation);
        assert_eq!(fm.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(fm.allowed_tools.as_deref(), Some("read_file, list_dir"));
        assert_eq!(body.trim(), "正文");
    }

    #[test]
    fn frontmatter_quoted_values_are_unquoted() {
        let raw = "---\ndescription: \"带: 冒号的描述\"\n---\n\n正文";
        let (fm, _) = parse_frontmatter(raw);
        assert_eq!(fm.description.as_deref(), Some("带: 冒号的描述"));
    }

    #[test]
    fn frontmatter_missing_keys_default() {
        let raw = "---\nname: s\n---\n\n正文";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(fm.description, None);
        assert!(!fm.disable_model_invocation, "缺失按缺省，不是 true");
        assert_eq!(fm.license, None);
        assert_eq!(fm.allowed_tools, None);
        assert_eq!(body.trim(), "正文");
    }

    #[test]
    fn frontmatter_boolean_only_accepts_true() {
        // 只有字面 `true` 生效；`yes` / `1` / 任意值都按缺省（false）
        for value in ["yes", "1", "True", "true-ish"] {
            let raw = format!("---\ndisable-model-invocation: {value}\n---\n\n正文");
            let (fm, _) = parse_frontmatter(&raw);
            assert!(!fm.disable_model_invocation, "值 {value} 不应生效");
        }
        let (fm, _) = parse_frontmatter("---\ndisable-model-invocation: true\n---\n\n正文");
        assert!(fm.disable_model_invocation);
    }

    /// 开头的破折号不是独立一行时（如四连横线 `----` 的水平线），不得被解释为 frontmatter。
    ///
    /// 剥离口径沿用历史的宽松前缀判定（正文照旧剥），但键的解释与 `name` 校验只认规范块
    /// ——否则一段正常 Markdown 会因正文里的 `name:` 字面文本被判名字不符而拒绝启动。
    #[test]
    fn non_standalone_dashes_are_not_treated_as_frontmatter() {
        // `----` 是水平线（4 个破折号），不是 frontmatter 起始标记
        let raw = "----\nname: 这是正文里的字面文本\n---\n\n正文";
        let (fm, _body) = parse_frontmatter(raw);
        assert_eq!(
            fm,
            SkillFrontmatter::default(),
            "非独立一行的破折号不得被解释为 frontmatter"
        );

        // 经 validate_names 也不应因此报错（目录名刻意与正文里的 name 不同）
        let home = tmp();
        write_skill(&root_of(home.path()), "grilling", raw);
        assert!(
            validate_names(&root_of(home.path())).is_ok(),
            "正文里的 name: 文本不得触发名字一致性校验"
        );
    }

    #[test]
    fn unclosed_frontmatter_is_not_an_error_and_body_still_reads() {
        let raw = "---\nname: s\ndescription: 未闭合\n\n正文仍然可读";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(
            fm,
            SkillFrontmatter::default(),
            "未闭合按无 frontmatter 处理"
        );
        assert_eq!(body.trim(), raw.trim(), "未闭合时原文即正文");

        // 经 resolve 也不报错（正文非空）
        let home = tmp();
        write_skill(&root_of(home.path()), "s", raw);
        let resolved = resolve(&root_of(home.path()), &["s".into()]).unwrap();
        assert!(resolved[0].body.as_deref().unwrap().contains("未闭合"));
    }

    #[test]
    fn skill_without_frontmatter_block_is_all_defaults() {
        let raw = "直接就是正文，没有 frontmatter";
        let (fm, body) = parse_frontmatter(raw);
        assert_eq!(fm, SkillFrontmatter::default());
        assert_eq!(body.trim(), raw);
    }

    #[test]
    fn frontmatter_name_matching_dir_passes_but_mismatch_fails() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "grilling",
            "---\nname: grilling\n---\n\n正文",
        );
        assert!(validate_names(&root_of(home.path())).is_ok());

        write_skill(
            &root_of(home.path()),
            "to-spec",
            "---\nname: something-else\n---\n\n正文",
        );
        let err = validate_names(&root_of(home.path())).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("to-spec") && msg.contains("something-else"),
            "{msg}"
        );
    }
}
