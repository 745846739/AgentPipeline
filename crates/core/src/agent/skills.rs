//! 技能（skill）发现与解析（决策 170 / 172，修订决策 47）。
//!
//! 决策 47 把 skill 定义为「用户机器上装了对应的外部工具」（PATH 可执行文件名），
//! 只把**名字**列进 system prompt。本模块在保留该语义的前提下扩展出第二类技能：
//! **携带 markdown 正文** 的知识型技能（与 MCP 的分工见 backlog §B.1：
//! skill 是知识/流程指引，MCP 是可调用能力）。
//!
//! 技能的**唯一身份是名字**——两类来源同名即同一个技能，用户文件覆盖内嵌默认：
//!
//! | 来源 | 判定 | 正文 |
//! |---|---|---|
//! | 内嵌默认 | [`EMBEDDED_SKILLS`]（决策 7 的内嵌 persona 先例，票 04 起退场） | 有 |
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
//!
//! **三态渲染**（决策 172④，票 05）：技能在 prompt 里的呈现分三种形态，由 [`SkillRender`]
//! 表达——全文态（正文进 prompt）、名字态（只列名字，正文交给 `Skill` 工具按需拉取）、
//! 目录态（[`catalogue`]：未被声明的可用技能，只给名字 + 描述，是渐进披露的落点）。
//! 形态由配置声明（[`SkillDecl`]）决定，**名字仍是唯一身份**：同一个技能换形态不换身份。

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

/// 技能注入形态（决策 172④，票 05）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillMode {
    /// 全文注入：正文进 system prompt（裸字符串的默认解释，也是今天的行为）。
    #[default]
    Full,
    /// 名字态：只把名字列进 prompt，正文由 `Skill` 工具按需拉取（票 06）。
    Name,
}

impl SkillMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillMode::Full => "full",
            SkillMode::Name => "name",
        }
    }

    /// 解析配置里的字面量；未知值 → `None`（由调用方报错并定位到阶段/节点）。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "full" => Some(SkillMode::Full),
            "name" => Some(SkillMode::Name),
            _ => None,
        }
    }
}

/// 一条技能声明（`string | {name, mode, trusted}` 混合数组的元素，决策 172④）。
///
/// 名字是唯一身份，`mode` / `trusted` 是同名技能的**形态**而非身份的一部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDecl {
    pub name: String,
    pub mode: SkillMode,
    /// 信任标记。未信任技能不得全文注入（写入侧拒绝，见
    /// [`crate::config::parse_skill_decls`]）。
    pub trusted: bool,
}

impl SkillDecl {
    /// 裸字符串的向后兼容解释（决策 172④）：`{mode: "full", trusted: false}`。
    ///
    /// 「未信任不得全文注入」这道门**只对显式对象声明生效**——裸字符串是信任概念
    /// 出现之前手写的配置行，若一并拒绝则今天所有配置行都会失效，与「零迁移」冲突。
    pub fn from_bare(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: SkillMode::Full,
            trusted: false,
        }
    }
}

/// 一个已解析技能的**渲染形态**（决策 172④，票 05：三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillRender {
    /// 全文态：`### {name}` + 正文进 system prompt。
    Full { body: String },
    /// 名字态：`- {name}`；正文由 `Skill` 工具按需拉取，**不进** system prompt。
    Name,
    /// 目录态：`- {name}: {description}`——仅在可用池、未被声明，是渐进披露的落点。
    Catalogue { description: Option<String> },
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

/// 解析后的技能：名字 + 渲染形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub name: String,
    pub render: SkillRender,
}

impl ResolvedSkill {
    /// 全文态（正文进 prompt）。
    pub fn full(name: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            render: SkillRender::Full { body: body.into() },
        }
    }

    /// 名字态（只有名字；正文由 `Skill` 工具按需拉取）。
    pub fn name_only(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            render: SkillRender::Name,
        }
    }

    /// 目录态（渐进披露：名字 + 描述）。
    pub fn catalogue(name: impl Into<String>, description: Option<String>) -> Self {
        Self {
            name: name.into(),
            render: SkillRender::Catalogue { description },
        }
    }
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

/// 解析声明的一组技能为「名字 + 渲染形态」，保持声明顺序、去重（决策 172④，票 05）。
///
/// `skills_root` 是技能根本身（默认 `{home}/skills`，`[skills] dir` 可覆盖）。
///
/// - `mode = full`：读正文，正文为空（用户文件写了空内容）→ [`Error::Config`]，
///   与 `persona_path` 同口径；
/// - `mode = name`：**不读正文**——名字态的正文由 `Skill` 工具按需拉取（票 06），
///   这里读进来反而会把正文带进 system prompt，破坏 `prompt_template_hash` 对
///   名字态的钝感（票 05 的显式要求）。
///
/// 工具型技能（PATH 可执行文件，无正文）在名字态与全文态下都只渲染名字：它本来就没有
/// 正文可注入，这是决策 47 的原语义。
pub fn resolve(skills_root: &Path, declared: &[SkillDecl]) -> Result<Vec<ResolvedSkill>> {
    let mut out: Vec<ResolvedSkill> = Vec::new();
    for decl in declared {
        if out.iter().any(|s| s.name == decl.name) {
            continue;
        }
        let render = match decl.mode {
            SkillMode::Name => SkillRender::Name,
            SkillMode::Full => match body_of(skills_root, &decl.name)? {
                // 全文态**也**展开兄弟文件（票 07）：否则全文态下兄弟引用仍是死指针。
                // 缺失即 fail fast——本函数在启动校验与 prompt 组装两处都用，残缺的技能包
                // 必须在启动时暴露，而不是让 agent 拿着少一节的正文开工。
                Some(body) => SkillRender::Full {
                    body: expand_siblings_strict(skills_root, &decl.name, &body)?,
                },
                None => SkillRender::Name, // 工具型技能：只有名字（决策 47）
            },
        };
        out.push(ResolvedSkill {
            name: decl.name.clone(),
            render,
        });
    }
    Ok(out)
}

/// 技能**目录**（渐进披露，决策 172④，票 05）：技能根下**未被声明**、且未被
/// `disable-model-invocation` 排除的 markdown 技能，渲染为 `- {name}: {description}`。
///
/// 三条准入：
/// - **只收技能根下的 markdown 技能**。工具型技能（PATH 可执行文件）没有正文可加载
///   ——列进「按需加载」的目录等于向模型广告它拿不到的能力，且 PATH 下可执行文件动辄
///   上千，与本段「省上下文」的初衷相反（决策 47 的工具型技能仍按原语义由阶段声明列出
///   名字）。内嵌技能同样不收：它正在退场（决策 172①，票 04），目录语义定在**最终形态**
///   上，票 04 删内嵌时本函数零改动——这正是 expand–contract 想要的收敛点。
/// - 被声明的技能由 [`resolve`] 以全文/名字态渲染，**不再重复出现在目录里**，否则同一
///   技能在 prompt 里出现两次。
/// - `disable-model-invocation: true` 不进目录（选型 D）：上游 27 个技能中 14 个带此键，
///   正是不该静默常驻的那批。
///
/// 目录态**不含正文**，因此对 `prompt_template_hash` 只是「有哪些技能可用」级别的敏感，
/// 与正文变更无关（决策 137 / 票 05）。
pub fn catalogue(skills_root: &Path, declared: &[String]) -> Vec<ResolvedSkill> {
    // 直接扫技能根，不走 [`discover`]——后者会顺带枚举整个 PATH 找可执行文件，而目录态
    // 只收 markdown 技能。本函数在**每次 agent attempt** 的 prompt 组装路径上，不该为
    // 一批注定被过滤掉的名字付目录扫描的代价。
    markdown_skill_paths(skills_root)
        .into_iter()
        .filter(|(name, _)| !declared.contains(name))
        .filter_map(|(name, path)| {
            let fm = read_frontmatter(&path);
            (!fm.disable_model_invocation).then(|| ResolvedSkill::catalogue(name, fm.description))
        })
        .collect()
}

/// 兄弟文件**一级**展开（决策 172③，票 07）：把正文里的相对 markdown 引用内联。
///
/// 上游技能用 `[tests.md](tests.md)`、`[UI.md](UI.md)` 这类引用指向同目录的兄弟文件
/// （`tdd/tests.md`、`prototype/UI.md`…）。这些文件此前既不进 prompt、agent 也读不到
/// ——文件工具被 [`crate::agent::file_policy::FileToolPolicy`] 锁在 worktree + 任务目录内，
/// 于是引用是**双向死指针**。展开走加载器而非放宽文件读根：技能根与 `{home}/data/`
/// （provider 密钥明文存储，决策 112）同父。
///
/// 三条规则：
/// - **只展开一级**（对齐 Agent Skills 规范「Keep file references one level deep」）：
///   被内联的文件里的引用**不再展开**，否则链式加载失控、体积不可预测；
/// - **目标必须在技能目录之内**：拒绝 `../` 穿越与绝对路径（[`resolve_sibling`]）；
/// - **非 `.md` 引用不展开**（`scripts/*.py` 保留原样）：本系统没有脚本执行语义，
///   内联一段 Python 反而让技能作者误以为它会被执行。
///
/// 缺失的引用**在展开结果里显式标注**而不是静默删除——这是给模型看的：它会知道这里少
/// 了一节，而不是以为技能就长这样。`Err` 只用于调用方明确要求「残缺即失败」的场景
/// （见 [`load_body_strict`]）。
pub fn expand_siblings(skills_root: &Path, name: &str, body: &str) -> String {
    let dir = skills_root.join(name);
    // 目录不可 canonicalize（技能不存在等）时按无引用处理——正文已由调用方校验过
    let Ok(canonical_dir) = dir.canonicalize() else {
        return body.to_string();
    };
    expand_once(&canonical_dir, name, body)
}

/// 同 [`expand_siblings`]，但**缺失的兄弟文件是错误**。
///
/// 用于启动校验与 `Skill` 工具路径：技能包残缺必须 fail fast / 明确报错，报文带
/// 技能名 + 缺失文件名（票 07 的验收项），否则技能作者看到的只是「少了一节」。
pub fn expand_siblings_strict(skills_root: &Path, name: &str, body: &str) -> Result<String> {
    let missing = missing_siblings(skills_root, name, body);
    if let Some(target) = missing.first() {
        return Err(Error::Config(format!(
            "技能 {name} 引用的兄弟文件缺失：{target}"
        )));
    }
    Ok(expand_siblings(skills_root, name, body))
}

/// 列出正文里引用了、但**读不到**的兄弟文件（按出现顺序）。
///
/// 与 [`expand_once`] 共用同一套「什么算兄弟引用」的判定：非 `.md` 不算、越界路径不算
/// （它们要么本就不该被展开，要么是恶意的，都不该被报成「缺失」）。
pub fn missing_siblings(skills_root: &Path, name: &str, body: &str) -> Vec<String> {
    let dir = skills_root.join(name);
    let Ok(canonical_dir) = dir.canonicalize() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for cap in sibling_link_regex().captures_iter(body) {
        let target = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if !target.ends_with(".md") {
            continue;
        }
        match resolve_sibling(&canonical_dir, target) {
            // 越界是拒绝展开，不是「缺失」——不报成缺文件
            SiblingTarget::OutOfBounds => {}
            SiblingTarget::InBounds(path) if !path.is_file() => out.push(target.to_string()),
            SiblingTarget::InBounds(_) => {}
        }
    }
    out
}

/// 实际的展开实现：`base` 是**已 canonicalize** 的技能目录。
fn expand_once(base: &Path, name: &str, body: &str) -> String {
    let re = sibling_link_regex();
    let mut out = String::with_capacity(body.len());
    let mut last = 0usize;
    for cap in re.captures_iter(body) {
        let whole = cap.get(0).expect("整体匹配");
        let target = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        // 非 .md 引用：原样保留（不展开、不执行）
        if !target.ends_with(".md") {
            continue;
        }
        // 越界路径：保留原样文本，不让它变成一次任意文件读
        let path = match resolve_sibling(base, target) {
            SiblingTarget::OutOfBounds => continue,
            SiblingTarget::InBounds(p) => p,
        };
        let replacement = match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let inner = parse_frontmatter(&raw).1.trim().to_string();
                // 内联内容里的引用**不再展开**（只一级）——原样带进结果
                format!(
                    "{}\n\n### 参考：{target}\n\n{inner}\n",
                    whole.as_str().trim_end()
                )
            }
            Err(_) => format!(
                "{}（⚠ 兄弟文件缺失：技能 {name} 引用的 {target} 读不到）",
                whole.as_str()
            ),
        };
        out.push_str(&body[last..whole.start()]);
        out.push_str(&replacement);
        last = whole.end();
    }
    out.push_str(&body[last..]);
    out
}

/// 兄弟文件的相对引用正则：`[任意文本](目标)`。
///
/// 目标里排掉 `#`（锚点）与空白；`/` 仍会匹配进来，由 [`resolve_sibling`] 统一拒绝——
/// 「同目录兄弟文件」的判定只在一处，正则不重复表达安全规则。
fn sibling_link_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\[[^\]]*\]\(([^)#\s]+)\)").expect("兄弟文件引用正则合法"))
}

/// 兄弟文件引用的解析结果。
enum SiblingTarget {
    /// 词法上落在技能目录内（文件是否存在另行判定）。
    InBounds(PathBuf),
    /// 越界：绝对路径、`..` 穿越，或符号链接指向目录之外。
    OutOfBounds,
}

/// 把引用目标解析成技能目录内的路径；越界返回 [`SiblingTarget::OutOfBounds`]。
///
/// **`base` 必须是已 canonicalize 的技能目录**（调用方保证）。
///
/// 两道判定缺一不可：
/// - **词法规范化**先消掉 `.` / `..`：文件可能还不存在，此时 `canonicalize` 会失败，
///   不能靠它做越界判定。逐段消费 path components，每走一步都要求仍在 `base` 之下。
/// - **canonicalize 复检**（文件存在时）兜住**符号链接逃逸**：字面上 `link.md` 就在技能
///   目录内，但它可能是一个指向 `/etc/passwd` 的软链。
///
/// 「不存在」与「越界」严格分开：前者是技能包残缺（要报错），后者是恶意/误写
/// （不展开、不读、不报缺失）。
fn resolve_sibling(base: &Path, target: &str) -> SiblingTarget {
    use std::path::Component;

    if target.is_empty() {
        return SiblingTarget::OutOfBounds;
    }
    let mut normalized = base.to_path_buf();
    for comp in Path::new(target).components() {
        match comp {
            Component::Normal(c) => normalized.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || !normalized.starts_with(base) {
                    return SiblingTarget::OutOfBounds;
                }
            }
            // 绝对路径（`/`）与平台前缀：一律越界
            Component::RootDir | Component::Prefix(_) => return SiblingTarget::OutOfBounds,
        }
    }
    if !normalized.starts_with(base) {
        return SiblingTarget::OutOfBounds;
    }
    match normalized.canonicalize() {
        // 存在：canonicalize 后再查一次，拦符号链接逃逸
        Ok(real) if !real.starts_with(base) => SiblingTarget::OutOfBounds,
        Ok(real) => SiblingTarget::InBounds(real),
        // 不存在：词法上在界内，交给缺失判定
        Err(_) => SiblingTarget::InBounds(normalized),
    }
}

/// 按名加载一个技能的正文（`Skill` 工具与名字态渲染的共用取数口，决策 172③，票 06）。
///
/// 与 [`resolve`] 的区别在**用途与失败语义**：
/// - `resolve` 在 prompt 组装路径上，声明过的名字必然已过启动校验，读不到就是坏配置；
/// - 本函数在**工具调用**路径上，名字来自模型（可能写错、可能指向未声明的技能），
///   因此返回 `Result` 让调用方把「找不到」变成给模型的错误文本，而非 fail fast——
///   模型据此自我纠正（票 06 的核心行为）。
///
/// 工具型技能（PATH 可执行文件）没有正文，报错时**说清是为什么**：只说「找不到」会让
/// 模型反复重试同一个名字。
///
/// **`disable-model-invocation: true` 的技能不加载**（票 06 / 选型 D）：该键的语义就是
/// 「别让模型自动调用我」。它与 `catalogue` 的过滤是同一条规则的两面——目录不广告它、
/// 工具也不给它开后门，否则模型从别处看到名字就能绕过这个开关。
///
/// **信任态是「声明」的属性而非技能文件的属性**（决策 172④⑥）：未信任技能**可以**被
/// 本工具加载（这正是 `mode: "name"` 的用法——不进 system prompt、按需拉取，见 spec §6），
/// 信任门约束的是**全文注入**那条路径，在 [`crate::config::parse_skill_decls`] 里把关。
pub fn load_body(skills_root: &Path, name: &str) -> Result<String> {
    // 先确认名字在可用池里，以便区分「没这个技能」与「有这个技能但它是工具型 / 被禁用」
    let known = discover(skills_root).into_iter().find(|s| s.name == name);
    match known {
        None => Err(Error::Config(format!("技能不存在：{name}"))),
        Some(s) if s.frontmatter.disable_model_invocation => Err(Error::Config(format!(
            "技能 {name} 声明了 disable-model-invocation，不允许模型自动调用"
        ))),
        Some(s) if matches!(s.source, SkillSource::Tool) => Err(Error::Config(format!(
            "技能 {name} 是 PATH 工具型技能，没有可注入的正文（它只有名字）"
        ))),
        Some(_) => {
            let body = body_of(skills_root, name)?.ok_or_else(|| {
                Error::Config(format!(
                    "技能 {name} 的正文不可读（既无用户文件也无内嵌正文）"
                ))
            })?;
            // 兄弟文件缺失 → 明确报错（票 07），报文带技能名 + 缺失文件名
            expand_siblings_strict(skills_root, name, &body)
        }
    }
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

    /// 裸字符串声明（按 `{mode: full, trusted: false}` 解释，决策 172④）。
    fn decls(names: &[&str]) -> Vec<SkillDecl> {
        names.iter().map(|n| SkillDecl::from_bare(*n)).collect()
    }

    /// 全文态声明的正文（断言用）。
    fn full_body(skill: &ResolvedSkill) -> &str {
        match &skill.render {
            SkillRender::Full { body } => body,
            other => panic!("期望全文态，实际 {other:?}"),
        }
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
        let resolved = resolve(&root_of(home.path()), &decls(&["grilling", "to-spec"])).unwrap();
        assert_eq!(resolved.len(), 2);
        // 正文是流水线原生版本：grilling 讲 pending 回路，to-spec 守决策 136
        let grilling = full_body(&resolved[0]);
        assert!(grilling.contains("info_insufficient"), "{grilling}");
        assert!(grilling.contains("frontier"), "{grilling}");
        let to_spec = full_body(&resolved[1]);
        assert!(to_spec.contains("验收标准"), "{to_spec}");
        assert!(to_spec.contains("acceptance_criteria"), "{to_spec}");
    }

    #[test]
    fn user_markdown_overrides_embedded_body() {
        let home = tmp();
        write_skill(&root_of(home.path()), "grilling", "用户自己的拷问流程");
        let resolved = resolve(&root_of(home.path()), &decls(&["grilling"])).unwrap();
        assert_eq!(full_body(&resolved[0]), "用户自己的拷问流程");
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
        let resolved = resolve(&root_of(home.path()), &decls(&["grilling"])).unwrap();
        assert_eq!(full_body(&resolved[0]), "正文从这里开始");
    }

    #[test]
    fn empty_user_file_is_config_error() {
        let home = tmp();
        write_skill(&root_of(home.path()), "grilling", "   \n");
        let err = resolve(&root_of(home.path()), &decls(&["grilling"])).unwrap_err();
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
        let resolved = resolve(&root_of(home.path()), &decls(&["win-skill"])).unwrap();
        let body = full_body(&resolved[0]);
        assert_eq!(body, "第一行\n第二行", "行尾应统一为 \\n：{body:?}");
        assert!(!body.contains('\r'), "{body:?}");
    }

    #[test]
    fn tool_skill_renders_as_name_only() {
        let home = tmp();
        // 工具型技能（PATH 可执行文件）不在技能根里，无正文——用未声明的名字验证。
        // 决策 47 原语义：只列名字（与「名字态」渲染相同，但语义是「无正文可注入」）。
        let resolved = resolve(
            &root_of(home.path()),
            &decls(&["definitely-not-a-knowledge-skill"]),
        )
        .unwrap();
        assert_eq!(resolved[0].render, SkillRender::Name);
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
            &decls(&["to-spec", "grilling", "to-spec"]),
        )
        .unwrap();
        assert_eq!(
            resolved.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["to-spec", "grilling"]
        );
    }

    // ── 票 05（决策 172④）：二档注入与三态渲染 ──

    /// 名字态**不读正文**：即便技能文件不存在也应成功（正文由 `Skill` 工具按需拉取）。
    ///
    /// 这是名字态相对全文态的关键差别——它把「技能是否可注入」与「文件是否在当前
    /// 技能根下」解耦。若这里读了正文，一个尚未安装的技能会让整个节点启动失败。
    #[test]
    fn name_mode_does_not_read_body() {
        let home = tmp();
        let resolved = resolve(
            &root_of(home.path()),
            &[SkillDecl {
                name: "not-installed".into(),
                mode: SkillMode::Name,
                trusted: true,
            }],
        )
        .unwrap();
        assert_eq!(resolved[0].render, SkillRender::Name);
    }

    /// 同一个技能，全文态读到正文、名字态不读——两档的差别正是「正文进不进 prompt」。
    #[test]
    fn full_and_name_modes_differ_only_in_body() {
        let home = tmp();
        write_skill(&root_of(home.path()), "s", "技能正文");
        let full = resolve(&root_of(home.path()), &[SkillDecl::from_bare("s")]).unwrap();
        assert_eq!(full_body(&full[0]), "技能正文");
        let name = resolve(
            &root_of(home.path()),
            &[SkillDecl {
                name: "s".into(),
                mode: SkillMode::Name,
                trusted: true,
            }],
        )
        .unwrap();
        assert_eq!(name[0].render, SkillRender::Name, "名字态不带正文");
    }

    /// 目录态（渐进披露）：未被声明的 markdown 技能 + `description`，且**不含正文**。
    #[test]
    fn catalogue_lists_undeclared_markdown_skills_with_description() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "available",
            "---\ndescription: 可用的技能\n---\n\n正文",
        );
        write_skill(&root_of(home.path()), "declared", "已声明的正文");
        let cat = catalogue(&root_of(home.path()), &["declared".to_string()]);
        assert_eq!(cat.len(), 1, "已声明的技能不进目录：{cat:?}");
        assert_eq!(cat[0].name, "available");
        assert_eq!(
            cat[0].render,
            SkillRender::Catalogue {
                description: Some("可用的技能".into())
            }
        );
    }

    /// 目录态**只含名字**——把正文塞进目录等于渐进披露失效（票 05 的核心动机）。
    #[test]
    fn catalogue_never_carries_body() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "big",
            "很长很长的正文".repeat(100).as_str(),
        );
        let cat = catalogue(&root_of(home.path()), &[]);
        for s in &cat {
            assert!(
                matches!(s.render, SkillRender::Catalogue { .. }),
                "目录项不得是全文态：{s:?}"
            );
        }
    }

    /// 选型 D：`disable-model-invocation: true` 的技能不进目录（不被自动注入）。
    #[test]
    fn disable_model_invocation_skills_are_excluded_from_catalogue() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "manual-only",
            "---\ndescription: 手动触发\ndisable-model-invocation: true\n---\n\n正文",
        );
        write_skill(
            &root_of(home.path()),
            "auto-ok",
            "---\ndescription: 可自动注入\n---\n\n正文",
        );
        let cat = catalogue(&root_of(home.path()), &[]);
        let names: Vec<&str> = cat.iter().map(|s| s.name.as_str()).collect();
        assert!(
            !names.contains(&"manual-only"),
            "带 disable-model-invocation 的技能不得进目录：{names:?}"
        );
        assert!(names.contains(&"auto-ok"), "{names:?}");
    }

    /// 工具型技能（PATH 可执行文件）不进目录：没有正文可加载，列进去是广告拿不到的能力。
    ///
    /// 用 `sh`（几乎必然存在于 PATH）验证——它绝不可能是技能根下的 markdown 技能。
    #[test]
    fn tool_type_skills_are_not_in_catalogue() {
        let home = tmp();
        // 前提：`sh` 确实是 PATH 里的可执行文件，否则本用例无意义
        assert!(
            skill_names(&root_of(home.path())).contains(&"sh".to_string()),
            "前提失败：PATH 里没有 sh"
        );
        let cat = catalogue(&root_of(home.path()), &[]);
        assert!(
            !cat.iter().any(|s| s.name == "sh"),
            "工具型技能不得进目录（无正文可加载）"
        );
    }

    /// 票 03：断言对象从内嵌常量换为用户目录技能——同一组行为在**用户文件**上成立。
    #[test]
    fn user_directory_skills_behave_like_embedded_did() {
        let home = tmp();
        write_skill(&root_of(home.path()), "my-grill", "把设计树走完再动手");
        write_skill(
            &root_of(home.path()),
            "my-spec",
            "综合成规格，验收标准要编号",
        );
        let resolved = resolve(&root_of(home.path()), &decls(&["my-grill", "my-spec"])).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(full_body(&resolved[0]).contains("设计树"));
        assert!(full_body(&resolved[1]).contains("验收标准"));
    }

    // ── 票 06（决策 172③）：按需加载正文（`Skill` 工具取数口） ──

    #[test]
    fn load_body_reads_user_skill() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "grill",
            "---\nname: grill\ndescription: x\n---\n\n拷问协议的正文",
        );
        let body = load_body(&root_of(home.path()), "grill").unwrap();
        assert_eq!(body, "拷问协议的正文", "frontmatter 须剥掉");
    }

    /// 未声明的技能也能加载——这是渐进披露的自动触发路径（票 06）。
    #[test]
    fn load_body_works_for_undeclared_skill() {
        let home = tmp();
        write_skill(&root_of(home.path()), "undeclared", "池子里的技能");
        assert_eq!(
            load_body(&root_of(home.path()), "undeclared").unwrap(),
            "池子里的技能"
        );
    }

    /// **未知技能名返回 `Err`（不是 panic、不是空串）**，由 `Skill` 工具转成给模型的
    /// 错误文本——模型据此自我纠正，而不是让节点 fail fast（票 06）。
    #[test]
    fn load_body_unknown_name_is_error_with_name() {
        let home = tmp();
        let err = load_body(&root_of(home.path()), "no-such-skill").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        assert!(err.to_string().contains("no-such-skill"), "{err}");
    }

    /// 工具型技能没有正文可注入——报错要说清原因，否则模型会反复重试同一个名字。
    #[test]
    fn load_body_rejects_tool_type_skill_with_reason() {
        let home = tmp();
        assert!(
            skill_names(&root_of(home.path())).contains(&"sh".to_string()),
            "前提失败：PATH 里没有 sh"
        );
        let err = load_body(&root_of(home.path()), "sh").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("工具型"), "须说明是工具型技能：{msg}");
    }

    /// `disable-model-invocation: true` 的技能不允许模型自动调用（选型 D）。
    ///
    /// 与 `catalogue` 的过滤是同一条规则的两面：目录不广告它，工具也不给它开后门。
    #[test]
    fn load_body_refuses_disable_model_invocation_skill() {
        let home = tmp();
        write_skill(
            &root_of(home.path()),
            "manual-only",
            "---\ndescription: 手动触发\ndisable-model-invocation: true\n---\n\n正文",
        );
        let err = load_body(&root_of(home.path()), "manual-only").unwrap_err();
        assert!(
            err.to_string().contains("disable-model-invocation"),
            "{err}"
        );
    }

    /// 内嵌技能的正文仍可加载（用户目录无同名文件时的兜底，票 04 前有效）。
    #[test]
    fn load_body_falls_back_to_embedded() {
        let home = tmp();
        let body = load_body(&root_of(home.path()), "grilling").unwrap();
        assert!(body.contains("frontier"), "{body}");
    }

    // ── 票 07：兄弟文件一级展开 ──

    /// 在技能目录下写一个兄弟文件（支持嵌套路径）。
    fn write_sibling(root: &Path, skill: &str, file: &str, content: &str) {
        let path = root.join(skill).join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn sibling_reference_is_inlined() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "tdd", "主文档：先写测试\n\n[tests.md](tests.md)\n");
        write_sibling(&root, "tdd", "tests.md", "# 测试写法\n\n一个用例一件事");
        let body = load_body(&root, "tdd").unwrap();
        assert!(body.contains("主文档：先写测试"), "{body}");
        assert!(body.contains("一个用例一件事"), "兄弟文件应被内联：{body}");
        assert!(body.contains("### 参考：tests.md"), "{body}");
    }

    /// **只展开一级**：被内联文件里的引用保持原样，不继续展开
    /// （否则链式加载失控，体积不可预测）。
    #[test]
    fn sibling_expansion_is_one_level_only() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "outer", "[mid.md](mid.md)\n");
        write_sibling(&root, "outer", "mid.md", "中层\n\n[deep.md](deep.md)\n");
        write_sibling(&root, "outer", "deep.md", "深层内容不该出现");
        let body = load_body(&root, "outer").unwrap();
        assert!(body.contains("中层"), "{body}");
        assert!(
            !body.contains("深层内容不该出现"),
            "深层引用不得被展开：{body}"
        );
        // 深层引用保留原样文本（让 reader 知道还有这一节）
        assert!(body.contains("[deep.md](deep.md)"), "{body}");
    }

    /// `../` 穿越被拒绝——保留原样文本，且**不读**技能目录外的文件。
    #[test]
    fn parent_traversal_is_not_expanded() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "evil", "[sec](../../secret.md)\n");
        // 技能根之外放一个「机密」文件
        std::fs::write(home.path().join("secret.md"), "机密内容").unwrap();
        let body = load_body(&root, "evil").unwrap();
        assert!(!body.contains("机密内容"), "不得读技能目录之外：{body}");
        assert!(body.contains("../../secret.md"), "引用保持原样：{body}");
    }

    /// 绝对路径引用同样不展开。
    #[test]
    fn absolute_path_reference_is_not_expanded() {
        let home = tmp();
        let root = root_of(home.path());
        let outside = home.path().join("outside.md");
        std::fs::write(&outside, "外部内容").unwrap();
        write_skill(&root, "abs", &format!("[x]({})\n", outside.display()));
        let body = load_body(&root, "abs").unwrap();
        assert!(!body.contains("外部内容"), "{body}");
    }

    /// 缺失的兄弟文件 → `Error::Config`，报文带**技能名 + 缺失文件名**（票 07 验收项）。
    #[test]
    fn missing_sibling_is_config_error_with_skill_and_file() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "broken", "[gone.md](gone.md)\n");
        let err = load_body(&root, "broken").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("broken"), "须含技能名：{msg}");
        assert!(msg.contains("gone.md"), "须含缺失文件名：{msg}");
    }

    /// 非 `.md` 引用**不展开、不执行**，保留原样文本。
    ///
    /// 本系统没有脚本执行语义，内联一段 Python 会让技能作者误以为它会被跑起来。
    #[test]
    fn non_markdown_reference_is_left_alone() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "scripty", "见 [run.py](scripts/run.py)\n");
        write_sibling(&root, "scripty", "scripts/run.py", "print('不该被内联')");
        let body = load_body(&root, "scripty").unwrap();
        assert!(
            body.contains("[run.py](scripts/run.py)"),
            "非 md 引用保持原样：{body}"
        );
        assert!(!body.contains("不该被内联"), "{body}");
    }

    /// 全文态**也**展开兄弟文件（票 07 的显式要求）——否则全文态下引用仍是死指针。
    #[test]
    fn full_mode_expansion_inlines_siblings() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "full-skill", "主文\n\n[sib.md](sib.md)\n");
        write_sibling(&root, "full-skill", "sib.md", "兄弟正文");
        let resolved = resolve(&root, &decls(&["full-skill"])).unwrap();
        let body = full_body(&resolved[0]);
        assert!(body.contains("兄弟正文"), "全文态也应展开：{body}");
    }

    /// 全文态下缺失的兄弟文件同样 fail fast（技能包残缺必须在启动时暴露）。
    #[test]
    fn full_mode_missing_sibling_is_config_error() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(&root, "full-broken", "[nope.md](nope.md)\n");
        let err = resolve(&root, &decls(&["full-broken"])).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("full-broken") && msg.contains("nope.md"),
            "{msg}"
        );
    }

    /// 名字态**不展开也不需要兄弟文件**——正文根本不在 prompt 里（票 05 与票 07 的交界）。
    #[test]
    fn name_mode_does_not_require_siblings() {
        let home = tmp();
        let root = root_of(home.path());
        write_skill(
            &root,
            "deferred",
            "[not-installed-yet.md](not-installed-yet.md)\n",
        );
        let resolved = resolve(
            &root,
            &[SkillDecl {
                name: "deferred".into(),
                mode: SkillMode::Name,
                trusted: true,
            }],
        )
        .unwrap();
        assert_eq!(resolved[0].render, SkillRender::Name);
    }

    /// 符号链接指向技能目录之外 → 不展开（canonicalize 后前缀比较兜住字面判定的漏洞）。
    #[cfg(unix)]
    #[test]
    fn symlink_escaping_skill_dir_is_not_expanded() {
        let home = tmp();
        let root = root_of(home.path());
        let outside = home.path().join("outside.md");
        std::fs::write(&outside, "外部机密").unwrap();
        write_skill(&root, "linky", "[escape.md](escape.md)\n");
        std::os::unix::fs::symlink(&outside, root.join("linky").join("escape.md")).unwrap();
        let body = load_body(&root, "linky").unwrap();
        assert!(!body.contains("外部机密"), "符号链接逃逸不得被展开：{body}");
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
        let resolved = resolve(external.path(), &decls(&["my-skill"])).unwrap();
        assert_eq!(full_body(&resolved[0]), "外部技能正文");
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
        let resolved = resolve(&root_of(home.path()), &decls(&["s"])).unwrap();
        assert!(full_body(&resolved[0]).contains("未闭合"));
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
