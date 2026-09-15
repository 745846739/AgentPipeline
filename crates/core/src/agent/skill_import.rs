//! 技能导入与卸载（决策 172⑤，票 09）。
//!
//! 用户能装技能的三个入口在这里收口：**上传 zip**、**导入一个本地技能目录**、**扫描一个本地
//! 技能根**（典型如 `~/.zcode/skills`）。三者产出同一种东西——一个 [`SkillPackage`]（技能名
//! 加上若干「相对路径 → 字节」的条目），因此**校验、同名冲突、落盘只实现一次**，zip 与目录
//! 只是两种取包方式。票 10 的远程 registry 复用同一入口（下载的 zip 走
//! [`SkillPackage::from_zip`]）。
//!
//! ## 落盘布局
//!
//! `{skills_root}/{name}/SKILL.md` + 兄弟文件（票 07 的展开依赖它们真的在磁盘上）。
//! `skills_root` 是技能根**本身**（默认 `{home}/skills`，`[skills] dir` 可覆盖，票 01）——
//! 本模块不拼 `skills` 目录名，与 [`crate::agent::skills`] 的每个入口同口径。
//!
//! ## 路径穿越是本模块的主要风险
//!
//! 包里的条目名来自**不可信输入**（用户上传的 zip / 指定的目录），因此两道门都不省：
//! - **[`SkillPackage::from_zip`]**：条目名先过 `zip` crate 的 `enclosed_name()`（它自带
//!   `..` 折叠与 NUL 拒绝，且其自身历史漏洞 GHSA-94vh-gphv-8pm8 正是「规范化不当导致任意
//!   写」），再按票面要求**显式拒绝绝对路径与 `..`**——`enclosed_name` 会**剥掉**前导 `/`
//!   而不是拒绝，对「上传的包不该有绝对路径」这件事太宽松；
//! - **[`SkillPackage::from_dir`]**：逐段走 [`sanitize_rel_path`]，与 zip 侧同一判定；
//! - **[`install`] 落盘前**实数校验目标落在技能根之内（`canonicalize` 后前缀比较，
//!   与 [`crate::agent::file_policy::FileToolPolicy`] 同口径的判定思路）。
//!
//! ## 同名冲突默认拒绝
//!
//! 技能名是唯一身份（决策 172），同名的第二份来源会让「这个技能是什么」变得不确定。因此
//! 冲突时**默认拒绝**，错误报文报出冲突技能名与**它当前的来源**（用户 markdown 路径 / PATH
//! 可执行文件），只有调用方显式传 `overwrite` 才覆盖。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::agent::skills::{parse_frontmatter, SkillSource, SKILL_FILE};
use crate::error::{Error, Result};

/// 单个技能包/条目的大小上限（16 MiB）。
///
/// 防止「解压炸弹」式的内存耗尽：全部条目都进内存，没有上限时一个压缩比极高的
/// zip 能把进程撑爆。技能是 markdown，最大的上游技能约 12 KiB，16 MiB 有三个数量级余量。
const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;

/// 一个包的条目总数上限（4096）。
///
/// 与 [`MAX_ENTRY_BYTES`] 同样防解压炸弹（大量极小条目同样能撑爆内存）。
const MAX_ENTRIES: usize = 4096;

/// 一个待导入的技能包：技能名 + 相对路径到内容的映射。
///
/// 条目路径一律是**相对技能目录**的（`SKILL.md`、`tests.md`、`scripts/run.py`），
/// 绝对路径与 `..` 在构造期就被拒绝，因此 [`install`] 可以放心 `join`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPackage {
    /// 技能名（也是落盘的目录名）。
    pub name: String,
    /// 相对路径 → 字节内容。用 `BTreeMap` 让落盘顺序稳定（便于测试与复现）。
    pub files: BTreeMap<String, Vec<u8>>,
}

/// 解析后的技能包元数据（导入校验的产物，供预览与界面用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    /// frontmatter 的 `description`（缺省 `None`）。
    pub description: Option<String>,
    /// 正文之外的兄弟文件数（票 07 的展开依赖它们）。
    pub sibling_count: usize,
}

impl SkillPackage {
    /// 从 zip 字节构造（票 09 的上传入口；票 10 的 registry 下载复用）。
    ///
    /// `name` 是技能名：传给它是**显式指定**（界面上的「技能名」字段），不传则从包的布局推断
    /// （`{name}/SKILL.md`）。两种真实打包方式都要能用——
    /// `zip -r grill.zip grill/`（带一层目录，可推断）与
    /// `cd grill && zip ../grill.zip -r .`（`SKILL.md` 在根，须显式给名），
    /// 后者推不出名字，故不以「根下 `SKILL.md`」为错误，而是要求调用方给名。
    ///
    /// 校验顺序刻意是「先条目、后结构」：条目名不合法（穿越 / 绝对路径）是**恶意输入**，
    /// 优先于「包里没有 `SKILL.md`」这种结构问题报出来。
    pub fn from_zip(bytes: &[u8], name: Option<&str>) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| Error::Validation(format!("zip 读取失败：{e}")))?;
        if archive.len() > MAX_ENTRIES {
            return Err(Error::Validation(format!(
                "zip 条目过多（{} > {MAX_ENTRIES}），拒绝解压",
                archive.len()
            )));
        }

        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| Error::Validation(format!("zip 第 {i} 个条目读取失败：{e}")))?;

            // 目录条目（以 `/` 结尾）只是骨架：父目录由文件自身创建，这里直接跳过。
            // **先于路径判定跳过**——目录条目从不参与落盘，其名字无需（也不该）参与校验。
            if entry.is_dir() {
                continue;
            }

            // ① 我们自己那道判定（先做，理由有二：报文精确——「绝对路径」「含 `..` 穿越」
            //    分得清；且不把安全结论寄托在第三方库的实现细节上）。
            let rel = sanitize_rel_path(entry.name()).map_err(|reason| {
                Error::Validation(format!("zip 条目名非法（{reason}）：{}", entry.name()))
            })?;

            // 归档噪声：macOS 的 `zip -r` 会塞进 `__MACOSX/` 与 `.DS_Store`。它们不是技能
            // 内容，留着会变成落进技能目录的「兄弟文件」（还会让票 07 的展开看到无关的 .md）。
            if is_archive_junk(&rel) {
                continue;
            }

            // 独立复检：`zip` crate 自带的 `enclosed_name()`（拒绝绝对路径与越界 `..`，
            // 且其自身历史漏洞 GHSA-94vh-gphv-8pm8 正是「规范化不当导致任意写」）。
            // 它与 ① 是**两道独立判定**：单点出错不会直接变成越界写，故不复用 ① 的结果。
            if entry.enclosed_name().is_none() {
                return Err(Error::Validation(format!(
                    "zip 条目名越界（`enclosed_name` 判定为不安全）：{}",
                    entry.name()
                )));
            }

            if entry.size() > MAX_ENTRY_BYTES as u64 {
                return Err(Error::Validation(format!(
                    "zip 条目过大（{} 字节 > {MAX_ENTRY_BYTES}）：{rel}",
                    entry.size()
                )));
            }
            let mut buf = Vec::new();
            // `take` 限制读取量：条目头里声明的大小可被伪造，先按上限读、再多读 1 字节探测超限，
            // 这样解压炸弹在**读完之前**就被截断，而不是先撑爆内存再判。
            let limit = entry.size().min(MAX_ENTRY_BYTES as u64);
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut entry, limit + 1), &mut buf)
                .map_err(|e| Error::Validation(format!("zip 条目解压失败（{rel}）：{e}")))?;
            if buf.len() > MAX_ENTRY_BYTES {
                return Err(Error::Validation(format!(
                    "zip 条目解压后过大（> {MAX_ENTRY_BYTES} 字节）：{rel}"
                )));
            }
            files.insert(rel, buf);
        }

        let name = match name {
            Some(n) => n.to_string(),
            None => name_of_entries(&files)?,
        };
        let pkg = Self::from_entries_with_name(name, files)?;
        pkg.validate_single_root()?;
        Ok(pkg)
    }

    /// 从一个本地技能目录构造（票 09 的目录导入入口）。
    ///
    /// `dir` 是技能目录**本身**（含 `SKILL.md`），递归收集其中的普通文件。符号链接一律跳过
    /// ——顺着链接可以把技能目录之外的文件读进包（与 [`crate::agent::skills`] 的兄弟文件
    /// 展开拒绝符号链接逃逸同一姿态）。
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        collect_dir(dir, dir, &mut files)?;
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Validation(format!("目录名不可用：{}", dir.display())))?
            .to_string();
        // 目录导入天然只有一个根（收的就是这一个目录），单根校验是空操作，出于对称仍走一遍
        Self::from_entries_with_name(name, files)
    }

    /// 从「名字 + 条目表」构造并做结构校验（zip 与目录两条路径的汇合点）。
    fn from_entries_with_name(name: String, files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        // 技能名会变成落盘的**单层**目录名，故用比条目路径更严的判定（见 [`sanitize_skill_name`]）
        let name = sanitize_skill_name(&name)
            .map_err(|reason| Error::Validation(format!("技能名非法（{reason}）：{name}")))?;
        if files.is_empty() {
            return Err(Error::Validation("技能包为空（没有任何文件）".into()));
        }
        Ok(SkillPackage { name, files })
    }

    /// 结构校验：须含 `SKILL.md`、frontmatter 可解析、正文非空（票 09 的验收项）。
    ///
    /// **只校验、不落盘**——先校验后安装，坏包不留半份痕迹。
    pub fn validate(&self) -> Result<PackageInfo> {
        let key = self
            .skill_md_key()
            .ok_or_else(|| Error::Validation(format!("技能包不含 {SKILL_FILE}：{}", self.name)))?;
        let raw = std::str::from_utf8(&self.files[key]).map_err(|e| {
            Error::Validation(format!(
                "技能 {name} 的 {SKILL_FILE} 不是合法 UTF-8：{e}",
                name = self.name
            ))
        })?;
        let (fm, body) = parse_frontmatter(raw);
        if body.trim().is_empty() {
            return Err(Error::Validation(format!(
                "技能 {} 的正文为空：{key}",
                self.name
            )));
        }
        // frontmatter 里写了 name 就必须与目录名一致（与启动校验同一条不变量，决策 172）：
        // 导入是这道校验的第一现场，不能等下次启动才暴露。
        if let Some(declared) = fm.name.as_deref() {
            if !declared.is_empty() && declared != self.name {
                return Err(Error::Validation(format!(
                    "技能 {} 的 frontmatter name 与技能名不一致：{declared}（\
                     名字是唯一身份，须与导入的技能名同名）",
                    self.name
                )));
            }
        }
        Ok(PackageInfo {
            name: self.name.clone(),
            description: fm.description,
            sibling_count: self.files.len().saturating_sub(1),
        })
    }

    /// 校验所有条目都在 `SKILL.md` 所在的那一层之下。
    ///
    /// 一个包里混进第二个顶层目录（比如打进了整个技能根，或误带上另一个技能）时，那些条目
    /// 会被摊平进本技能目录，变成一堆来路不明的「兄弟文件」。这里明确拒绝，而不是猜用户想装哪个。
    fn validate_single_root(&self) -> Result<()> {
        let prefix = self.strip_prefix();
        if prefix.is_empty() {
            return Ok(());
        }
        if let Some(stray) = self.files.keys().find(|k| !k.starts_with(&prefix)) {
            return Err(Error::Validation(format!(
                "技能包混入了 {prefix} 之外的条目（{stray}）；\
                 一个包只应含一个技能——请只打包该技能目录"
            )));
        }
        Ok(())
    }

    /// 本包中 `SKILL.md` 相对路径（支持两种布局：根下 `SKILL.md`，或单层 `{name}/SKILL.md`）。
    ///
    /// 两种都收是因为从真实生态里打包时两种都常见：手工 `zip -r skill.zip skill/` 会带一层
    /// 目录，而直接对 `SKILL.md` 打包则不会。**多于一层不认**——那说明包的是整个技能根，
    /// 不是单个技能。
    fn skill_md_key(&self) -> Option<&String> {
        if self.files.contains_key(SKILL_FILE) {
            return self.files.get_key_value(SKILL_FILE).map(|(k, _)| k);
        }
        self.files.keys().find(|k| {
            Path::new(k.as_str())
                .parent()
                .is_some_and(|p| p.components().count() == 1)
                && Path::new(k.as_str())
                    .file_name()
                    .is_some_and(|f| f == SKILL_FILE)
        })
    }

    /// 落盘时所有条目共用的前缀（`SKILL.md` 在根时为 `""`，否则为 `{name}/`）。
    ///
    /// 单独一层目录被**剥掉**：包里的 `skill/SKILL.md` 落到 `{root}/{skill}/SKILL.md`，
    /// 而不是 `{root}/{skill}/skill/SKILL.md`。
    fn strip_prefix(&self) -> String {
        match self.skill_md_key() {
            Some(key) if key != SKILL_FILE => {
                let prefix = Path::new(key)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                format!("{prefix}/")
            }
            _ => String::new(),
        }
    }
}

/// 从条目表推出技能名：以 `SKILL.md` 所在目录为名。
///
/// 只在**未显式给名**时调用（zip 上传路径）。`SKILL.md` 在包根时推不出名字——此时要求调用方
/// 显式给名（`cd grill && zip ../grill.zip -r .` 这种打包方式很常见），而不是拿上传文件名
/// 之类不可信输入当身份。
fn name_of_entries(files: &BTreeMap<String, Vec<u8>>) -> Result<String> {
    let key = files
        .keys()
        .find(|k| {
            Path::new(k.as_str())
                .file_name()
                .is_some_and(|f| f == SKILL_FILE)
                && Path::new(k.as_str())
                    .parent()
                    .is_some_and(|p| p.components().count() == 1)
        })
        .ok_or_else(|| {
            if files.contains_key(SKILL_FILE) {
                return Error::Validation(format!(
                    "技能包的名字无法推断（{SKILL_FILE} 在包根）：请显式指定技能名，\
                     或把技能放进以其名字命名的目录后再打包"
                ));
            }
            // 更深嵌套（`a/b/SKILL.md`）不是「没有 SKILL.md」——包里有，只是布局多了一层。
            // 报成「不含 SKILL.md」会让用户以为包本身坏了，实际只需重新打包。
            if let Some(deep) = files.keys().find(|k| {
                Path::new(k.as_str())
                    .file_name()
                    .is_some_and(|f| f == SKILL_FILE)
            }) {
                return Error::Validation(format!(
                    "技能包的目录层级过深（{deep}）：一个包只应含一个技能，\
                     且 {SKILL_FILE} 须在包根或单层目录下"
                ));
            }
            Error::Validation(format!("技能包不含 {SKILL_FILE}"))
        })?;
    let name = Path::new(key)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Validation("技能名不是合法 UTF-8".into()))?;
    Ok(name.to_string())
}

/// 校验并归一一个**相对**条目路径；不合法时返回中文原因。
///
/// 拒绝：绝对路径、`..`、空路径、NUL、Windows 盘符前缀。`.` 被忽略（`./SKILL.md` ≡ `SKILL.md`）。
/// 返回的字符串统一用 `/` 分隔（zip 与 markdown 引用的通用写法）。
fn sanitize_rel_path(raw: &str) -> std::result::Result<String, String> {
    if raw.is_empty() {
        return Err("空路径".into());
    }
    if raw.contains('\0') {
        return Err("含 NUL 字节".into());
    }
    // Windows 风格分隔符归一后再判定，否则 `..\..\x` 能绕过 `/` 的检查
    let unified = raw.replace('\\', "/");
    if unified.starts_with('/') {
        return Err("绝对路径".into());
    }
    let mut out: Vec<&str> = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err("含 `..` 穿越".into()),
            // 盘符（`C:`）在 Unix 上不是前缀，但包可能来自 Windows，一律拒绝
            p if p.ends_with(':') => return Err("含盘符前缀".into()),
            p => out.push(p),
        }
    }
    if out.is_empty() {
        return Err("空路径".into());
    }
    // 逐段确认仍是纯普通组件（兜住上面没覆盖到的平台特有形态）
    for part in &out {
        let mut comps = Path::new(part).components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => {}
            _ => return Err(format!("非法路径段：{part}")),
        }
    }
    Ok(out.join("/"))
}

/// 校验并归一一个**技能名**；不合法时返回中文原因。
///
/// 比 [`sanitize_rel_path`] 更严的一条：**不得含路径分隔符**。技能名是落盘的**单层**目录名，
/// 而技能的发现（[`crate::agent::skills::discover`]）只扫技能根下**一层**目录。若允许
/// `a/b` 这样的名字，就会出现最坏的一种半成品状态——导入返回成功、文件也写下去了，但
/// `GET /skills` 列不出它、启动校验看不见它、executor 永远不会加载它（于用户是**静默失效**），
/// 而卸载又因为拒绝含分隔符的名字而删不掉。三个入口必须用**同一条**名字不变量。
fn sanitize_skill_name(raw: &str) -> std::result::Result<String, String> {
    let name = sanitize_rel_path(raw)?;
    if name.contains('/') {
        return Err("不得含路径分隔符（技能名是技能根下的单层目录名）".into());
    }
    Ok(name)
}

/// 校验一个技能名能否用作技能根下的目录名；不合法时返回中文原因。
///
/// 供**落盘之外的入口**复用同一条名字不变量——票 10 的市场路径要在**下载之前**就否掉一个
/// 不可能落盘的名字（否则白烧一次下载，最后才在落盘时失败）。判定用的就是 `install` 内部
/// 同一个 [`sanitize_skill_name`]，故两处不会漂移。
pub fn check_skill_name(raw: &str) -> std::result::Result<String, String> {
    sanitize_skill_name(raw)
}

/// 归档噪声：macOS 打包产生的元数据，不属于技能内容。
///
/// 留着它们会被当成兄弟文件落进技能目录，票 07 的展开还会对 `.DS_Store` 之类做无意义的
/// 扫描。判定按**路径段**（不是子串）——一个名叫 `my__MACOSX_notes.md` 的技能文件不该被误杀。
fn is_archive_junk(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    rel.split('/').any(|seg| seg == "__MACOSX") || name == ".DS_Store" || name == "Thumbs.db"
}

/// 递归收集目录下的普通文件（跳过符号链接，见 [`SkillPackage::from_dir`]）。
fn collect_dir(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::Validation(format!("目录不可读：{}（{e}）", dir.display())))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| Error::Validation(format!("目录项读取失败（{}）：{e}", dir.display())))?;
        let path = entry.path();
        // 符号链接不跟随：顺着它能把技能目录之外的文件读进包
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| Error::Validation(format!("无法读取 {}：{e}", path.display())))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_dir(base, &path, out)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .map_err(|_| Error::Validation(format!("路径不在技能目录内：{}", path.display())))?;
        let rel = sanitize_rel_path(&rel.to_string_lossy()).map_err(|reason| {
            Error::Validation(format!("条目路径非法（{reason}）：{}", rel.display()))
        })?;
        if is_archive_junk(&rel) {
            continue;
        }
        if meta.len() > MAX_ENTRY_BYTES as u64 {
            return Err(Error::Validation(format!(
                "文件过大（{} 字节 > {MAX_ENTRY_BYTES}）：{rel}",
                meta.len()
            )));
        }
        if out.len() >= MAX_ENTRIES {
            return Err(Error::Validation(format!(
                "文件数过多（> {MAX_ENTRIES}），拒绝导入"
            )));
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Validation(format!("文件读取失败（{rel}）：{e}")))?;
        out.insert(rel, bytes);
    }
    Ok(())
}

/// 同名冲突时已存在的那份技能的来源（错误报文与冲突分类用）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExistingSource {
    /// 技能根下的 markdown 技能——可被 `overwrite` 替换。
    Markdown(String),
    /// PATH 中的可执行文件（工具型技能）——不可替换，只能改名。
    Tool,
}

impl ExistingSource {
    fn describe(&self) -> String {
        match self {
            ExistingSource::Markdown(desc) => desc.clone(),
            ExistingSource::Tool => "PATH 中的同名可执行文件（工具型技能）".to_string(),
        }
    }
}

/// 查同名技能是否已存在及其来源（一次 `discover`，避免调用点重复扫描）。
fn existing_source(skills_root: &Path, name: &str) -> Option<ExistingSource> {
    crate::agent::skills::discover(skills_root)
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| match s.source {
            SkillSource::Markdown { path } => {
                ExistingSource::Markdown(format!("技能根下的 {}", path.display()))
            }
            SkillSource::Tool => ExistingSource::Tool,
        })
}

/// 把包安装到技能根（票 09）：校验 → 同名冲突判定 → 落盘。
///
/// `overwrite = false`（默认）时同名即 [`Error::Conflict`]，报文报出冲突技能名与其当前来源；
/// `true` 时先删掉旧目录再落新包（覆盖是显式确认后的动作，不留旧兄弟文件的残骸）。
///
/// **工具型技能的冲突不受 `overwrite` 影响**：`overwrite` 的语义是「替换技能根里那份同名技能」，
/// 而 PATH 里的可执行文件不归技能根管——删不掉它，装一份同名 markdown 只会把它的名字**遮住**
/// （知识型优先于工具型），用户以为覆盖了、其实 PATH 里那份还在。这属于必须让用户改名的情形，
/// 故一律 [`Error::Conflict`]。
///
/// 落盘前对目标目录做 **realpath 校验**（`canonicalize` 后确认仍在技能根内）——技能根自身
/// 可能是符号链接（如指向另一磁盘），因此比较的是**双方的 realpath**，而不是让技能根保持
/// 未解析形态去做前缀匹配。
pub fn install(skills_root: &Path, package: &SkillPackage, overwrite: bool) -> Result<PackageInfo> {
    let info = package.validate()?;

    if let Some(existing) = existing_source(skills_root, &package.name) {
        // 工具型技能的冲突不受 `overwrite` 影响（见函数文档）
        if existing == ExistingSource::Tool {
            return Err(Error::Conflict(format!(
                "技能 {} 与 PATH 中的可执行文件同名；\
                 覆盖它需要改名——PATH 文件不归本系统管，装同名知识型技能只会把它遮住",
                package.name
            )));
        }
        if !overwrite {
            return Err(Error::Conflict(format!(
                "技能 {} 已存在（当前来源：{}）；覆盖需显式确认",
                package.name,
                existing.describe()
            )));
        }
    }

    let target = skills_root.join(&package.name);

    // 技能根不存在时建立它。默认根由 `Home::ensure_dirs` 建好，但 `[skills] dir` 覆盖的根
    // **有意不建**（决策 172：那是用户自己的生态目录，家目录骨架不得新建或改权限）。
    // 于是「配置指向一个尚未存在的目录」是合法配置，若这里也拒绝，用户在没有 shell 的
    // 桌面 / 网页形态下就**永远装不进第一个技能**。导入是用户对「装到该根」的显式动作，
    // 建根是这次动作的一部分——配置指向哪里，技能就装在哪里，没有歧义。
    if !skills_root.exists() {
        std::fs::create_dir_all(skills_root).map_err(|e| {
            Error::Validation(format!("技能根不可创建：{}（{e}）", skills_root.display()))
        })?;
    }
    ensure_inside(skills_root, &target)?;

    // 覆盖：整目录换掉。只删技能目录本身，不碰技能根。
    if overwrite && target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| {
            Error::Validation(format!("覆盖技能 {} 前无法清理旧目录：{e}", package.name))
        })?;
    }

    let prefix = package.strip_prefix();
    for (rel, bytes) in &package.files {
        let rel = rel.strip_prefix(&prefix).unwrap_or(rel);
        let rel = sanitize_rel_path(rel)
            .map_err(|reason| Error::Validation(format!("条目路径非法（{reason}）：{rel}")))?;
        let dest = target.join(&rel);
        ensure_inside(skills_root, &dest)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Validation(format!("无法创建目录 {}：{e}", parent.display()))
            })?;
        }
        std::fs::write(&dest, bytes)
            .map_err(|e| Error::Validation(format!("写入 {} 失败：{e}", dest.display())))?;
    }

    Ok(info)
}

/// 落盘前确认 `target` 解析后仍在 `skills_root` 之内（票 09 的 realpath 要求）。
///
/// 目标可能尚不存在（新装技能），因此**逐级向上找最近的已存在祖先**做 `canonicalize`，
/// 再拼回未创建的部分——直接对不存在的路径 `canonicalize` 会失败，而失败不能当作「安全」。
fn ensure_inside(skills_root: &Path, target: &Path) -> Result<()> {
    let root = skills_root.canonicalize().map_err(|e| {
        Error::Validation(format!("技能根不可用：{}（{e}）", skills_root.display()))
    })?;

    let mut existing = target.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(real) => {
                let resolved = tail.iter().rev().fold(real, |acc, part| acc.join(part));
                if !resolved.starts_with(&root) {
                    return Err(Error::Validation(format!(
                        "拒绝写入技能根之外：{}（解析为 {}）",
                        target.display(),
                        resolved.display()
                    )));
                }
                return Ok(());
            }
            Err(_) => {
                let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                    return Err(Error::Validation(format!(
                        "路径无法解析：{}",
                        target.display()
                    )));
                };
                tail.push(name);
                if !existing.pop() {
                    return Err(Error::Validation(format!(
                        "路径无法解析：{}",
                        target.display()
                    )));
                }
            }
        }
    }
}

/// 卸载技能：删掉技能根下的 `{name}/`（票 09）。
///
/// **不检查引用**——技能名是唯一身份，卸载后引用它的阶段配置由启动校验与
/// `PUT /stage-configs` 的 fail fast 兜住（票面显式要求「不得静默降级」）。把引用检查放进
/// 卸载会制造一个隐蔽的依赖：想卸载得先改配置、想改配置得先卸载。
///
/// 技能不存在 → [`Error::Task`]（API 层映射 404）。工具型技能（PATH 可执行文件）不可卸载
/// ——它不归我们管，删用户的 PATH 文件是灾难。
pub fn uninstall(skills_root: &Path, name: &str) -> Result<()> {
    let name = sanitize_skill_name(name)
        .map_err(|reason| Error::Validation(format!("技能名非法（{reason}）：{name}")))?;
    let target = skills_root.join(&name);
    if !target.is_dir() {
        // 区分「工具型技能」与「不存在」：前者要明确拒绝，否则用户以为删掉了
        if let Some(source) = existing_source(skills_root, &name) {
            return Err(Error::Validation(format!(
                "技能 {name} 不是可卸载的本地技能（当前来源：{}）",
                source.describe()
            )));
        }
        return Err(Error::Task(format!("技能不存在：{name}")));
    }
    ensure_inside(skills_root, &target)?;
    std::fs::remove_dir_all(&target)
        .map_err(|e| Error::Validation(format!("卸载技能 {name} 失败：{e}")))?;
    Ok(())
}

/// 扫描一个本地技能根下的全部技能（票 09 的目录扫描）。
///
/// 返回**名字 + 描述 + 是否已在目标技能根里存在**，供界面逐个预览与勾选。
///
/// 与 [`crate::agent::skills::discover`] 的区别：那个是「本机可用技能」的权威列表（含 PATH
/// 工具型技能），本函数回答的是「这个目录里有什么可导入的」——因此**只扫 markdown 技能**
/// （工具型技能没有包可拷），且**不读 PATH**（扫描的是给定目录，不是本机环境）。
///
/// `target_root` 是判断「是否已存在」的参照技能根；传 `None` 表示不判重（`exists` 一律 false）。
/// 目录里**不含** `SKILL.md` 的子目录被跳过（那不是技能），而非报错——用户目录里杂物很多，
/// 扫描是「列出能导入的」，不是「校验这个目录」。
pub fn scan_root(source_root: &Path, target_root: Option<&Path>) -> Result<Vec<ScanEntry>> {
    let entries = std::fs::read_dir(source_root)
        .map_err(|e| Error::Validation(format!("目录不可读：{}（{e}）", source_root.display())))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            Error::Validation(format!("目录项读取失败（{}）：{e}", source_root.display()))
        })?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        let skill_md = dir.join(SKILL_FILE);
        if !skill_md.is_file() {
            continue;
        }
        let description = std::fs::read_to_string(&skill_md)
            .ok()
            .and_then(|raw| parse_frontmatter(&raw).0.description);
        // 已存在的判定走**同一个** discover 口，与启动校验、PUT /stage-configs 同源
        let exists = target_root
            .map(|root| existing_source(root, &name).is_some())
            .unwrap_or(false);
        out.push(ScanEntry {
            name,
            description,
            exists,
            path: skill_md,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 扫描结果的一行（票 09：名字 + 描述 + 是否已存在）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub name: String,
    pub description: Option<String>,
    /// 目标技能根里已有同名技能（导入会撞冲突，需显式覆盖）。
    pub exists: bool,
    /// `SKILL.md` 的绝对路径（界面提示用）。
    pub path: PathBuf,
}

/// 从一个已扫描的技能目录导入（扫描后的逐项导入调用）。
pub fn install_from_dir(
    skills_root: &Path,
    source_dir: &Path,
    overwrite: bool,
) -> Result<PackageInfo> {
    let package = SkillPackage::from_dir(source_dir)?;
    install(skills_root, &package, overwrite)
}

/// 批量导入：对每个来源目录各自导入，**逐项返回结果**，不因一项失败中断整批（票 09）。
///
/// 返回顺序与输入一致，`Err` 的项带该项自己的原因（同名冲突 / 缺 `SKILL.md` / 路径穿越…）。
pub fn install_batch(
    skills_root: &Path,
    sources: &[PathBuf],
    overwrite: bool,
) -> Vec<(PathBuf, Result<PackageInfo>)> {
    sources
        .iter()
        .map(|src| (src.clone(), install_from_dir(skills_root, src, overwrite)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_file(root: &Path, rel: &str, content: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    /// 用 zip 写一个包（测试专用：正向验证 from_zip 的读路径）。
    fn make_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::SimpleFileOptions = Default::default();
            for (name, content) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(content.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    /// 技能目录 fixture：`{root}/{name}/SKILL.md` + 可选兄弟文件。
    fn skill_dir(parent: &Path, name: &str, body: &str) -> PathBuf {
        let dir = parent.join(name);
        write_file(&dir, SKILL_FILE, body);
        dir
    }

    // ── 结构校验（票 09：须含 SKILL.md、frontmatter 可解析、正文非空）──

    #[test]
    fn zip_with_skill_md_installs() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let zip = make_zip(&[
            (
                "grill/SKILL.md",
                "---\ndescription: 拷问\n---\n\n拷问协议正文",
            ),
            ("grill/tests.md", "兄弟文件"),
        ]);
        let pkg = SkillPackage::from_zip(&zip, None).unwrap();
        assert_eq!(pkg.name, "grill");
        let info = install(&root, &pkg, false).unwrap();
        assert_eq!(info.name, "grill");
        assert_eq!(info.description.as_deref(), Some("拷问"));
        assert_eq!(info.sibling_count, 1);

        // 落盘布局：{root}/{name}/SKILL.md + 兄弟文件（票 07 依赖）
        assert!(root.join("grill/SKILL.md").is_file());
        assert!(root.join("grill/tests.md").is_file());
        // 单层目录被剥掉，不是 `{root}/grill/grill/SKILL.md`
        assert!(!root.join("grill/grill").exists());
        // 装完就是「本机可用技能」
        assert!(crate::agent::skills::skill_names(&root).contains(&"grill".to_string()));
    }

    /// 缺 `SKILL.md` → 拒绝，且报文说清缺什么。
    #[test]
    fn package_without_skill_md_is_rejected() {
        let zip = make_zip(&[("grill/notes.md", "只有笔记")]);
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "{err:?}");
        assert!(err.to_string().contains(SKILL_FILE), "{err}");
    }

    /// 平铺打包（`cd grill && zip -r ../grill.zip .`）须显式给名——第二种真实打包方式。
    #[test]
    fn flat_zip_installs_with_explicit_name() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let zip = make_zip(&[("SKILL.md", "平铺打包的正文"), ("tests.md", "兄弟文件")]);

        // 不给名：明确报错而不是猜一个名字
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(err.to_string().contains("显式指定技能名"), "{err}");

        let pkg = SkillPackage::from_zip(&zip, Some("flat")).unwrap();
        assert_eq!(pkg.name, "flat");
        install(&root, &pkg, false).unwrap();
        assert!(root.join("flat/SKILL.md").is_file());
        assert!(
            root.join("flat/tests.md").is_file(),
            "平铺包的兄弟文件一并落盘"
        );
    }

    /// 一个包里混入第二个顶层目录 → 拒绝（否则那些条目会被摊平成本技能的兄弟文件）。
    #[test]
    fn multi_root_zip_is_rejected() {
        let zip = make_zip(&[
            ("one/SKILL.md", "第一个技能"),
            ("two/SKILL.md", "第二个技能"),
        ]);
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(err.to_string().contains("只应含一个技能"), "{err}");
    }

    /// macOS 打包噪声（`__MACOSX/` 与 `.DS_Store`）被剔除，不落进技能目录。
    #[test]
    fn archive_junk_is_dropped() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let zip = make_zip(&[
            ("grill/SKILL.md", "正文"),
            ("grill/.DS_Store", "垃圾"),
            ("__MACOSX/grill/._SKILL.md", "垃圾"),
        ]);
        let pkg = SkillPackage::from_zip(&zip, None).unwrap();
        let info = install(&root, &pkg, false).unwrap();
        assert_eq!(info.sibling_count, 0, "噪声不算兄弟文件");
        assert!(!root.join("grill/.DS_Store").exists());
        assert!(!root.join("__MACOSX").exists());
    }

    /// 反向对照：名字里恰好含 `__MACOSX` 的正常技能文件不该被误杀（判定按路径段，不按子串）。
    #[test]
    fn junk_filter_does_not_eat_lookalike_names() {
        assert!(!is_archive_junk("grill/my__MACOSX_notes.md"));
        assert!(!is_archive_junk("grill/Thumbs.db.md"));
        assert!(is_archive_junk("__MACOSX/x"));
        assert!(is_archive_junk("a/__MACOSX/b"));
        assert!(is_archive_junk("grill/.DS_Store"));
    }

    /// 正文为空 → 拒绝（与 `resolve` 的 fail fast 同口径）。
    #[test]
    fn empty_body_is_rejected() {
        let zip = make_zip(&[("s/SKILL.md", "---\nname: s\n---\n\n   \n")]);
        let pkg = SkillPackage::from_zip(&zip, None).unwrap();
        let err = pkg.validate().unwrap_err();
        assert!(err.to_string().contains("正文为空"), "{err}");
    }

    /// frontmatter 的 `name` 与技能名不一致 → 拒绝（决策 172 的不变量，导入是第一现场）。
    #[test]
    fn mismatched_frontmatter_name_is_rejected() {
        let zip = make_zip(&[("s/SKILL.md", "---\nname: other\n---\n\n正文")]);
        let pkg = SkillPackage::from_zip(&zip, None).unwrap();
        let err = pkg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("other") && msg.contains("s"), "{msg}");
    }

    /// frontmatter 里没有 `name` 是合法的（四键都可缺省）。
    #[test]
    fn missing_frontmatter_name_is_fine() {
        let zip = make_zip(&[("s/SKILL.md", "---\ndescription: 描述\n---\n\n正文")]);
        let pkg = SkillPackage::from_zip(&zip, None).unwrap();
        assert_eq!(pkg.validate().unwrap().description.as_deref(), Some("描述"));
    }

    // ── 路径穿越（票 09 的主要风险）──

    #[test]
    fn zip_parent_traversal_is_rejected() {
        let zip = make_zip(&[("../evil.md", "恶意内容")]);
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "{err:?}");
        assert!(err.to_string().contains("穿越"), "{err}");
    }

    #[test]
    fn zip_absolute_path_is_rejected() {
        let zip = make_zip(&[("/etc/evil.md", "恶意内容")]);
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(err.to_string().contains("绝对路径"), "{err}");
    }

    /// 深层穿越：`a/../../evil.md` —— `enclosed_name` 与我们的判定都须拦下。
    #[test]
    fn zip_deep_traversal_is_rejected() {
        let zip = make_zip(&[("skill/../../evil.md", "恶意内容")]);
        assert!(SkillPackage::from_zip(&zip, None).is_err());
    }

    /// 反斜杠伪装的穿越（Windows 风格分隔符）同样拒绝。
    #[test]
    fn zip_backslash_traversal_is_rejected() {
        let zip = make_zip(&[("..\\..\\evil.md", "恶意内容")]);
        assert!(SkillPackage::from_zip(&zip, None).is_err());
    }

    /// 关键安全断言：穿越的包**没有在技能根之外留下任何文件**。
    #[test]
    fn traversal_package_writes_nothing_outside_root() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let zip = make_zip(&[("s/SKILL.md", "正文"), ("../escaped.md", "逃逸")]);
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "{err:?}");
        assert!(!home.path().join("escaped.md").exists());
        assert!(!root.join("escaped.md").exists());
    }

    /// 目录导入时符号链接不跟随——否则顺链接可把目录外文件读进包。
    #[cfg(unix)]
    #[test]
    fn directory_import_skips_symlinks() {
        let home = tmp();
        let secret = write_file(home.path(), "secret.md", "技能目录之外的机密");
        let dir = skill_dir(home.path(), "linky", "正文");
        std::os::unix::fs::symlink(&secret, dir.join("escape.md")).unwrap();

        let pkg = SkillPackage::from_dir(&dir).unwrap();
        assert!(
            !pkg.files.keys().any(|k| k == "escape.md"),
            "符号链接不得被收进包：{:?}",
            pkg.files.keys().collect::<Vec<_>>()
        );
    }

    /// 目录导入的条目路径同样过 `sanitize_rel_path`（对照断言，防日后放宽）。
    #[test]
    fn sanitize_rejects_traversal_forms() {
        for bad in [
            "../x",
            "/abs",
            "a/../../b",
            "..\\x",
            "",
            "./",
            "C:/x",
            "a\0b",
        ] {
            assert!(
                sanitize_rel_path(bad).is_err(),
                "{bad:?} 应被拒绝，实际通过"
            );
        }
        for ok in ["SKILL.md", "./SKILL.md", "a/b.md", "scripts/run.py"] {
            assert!(sanitize_rel_path(ok).is_ok(), "{ok:?} 应通过");
        }
        assert_eq!(sanitize_rel_path("./SKILL.md").unwrap(), "SKILL.md");
        assert_eq!(sanitize_rel_path("a//b.md").unwrap(), "a/b.md");
    }

    // ── 同名冲突（票 09：默认拒绝并报出来源，覆盖需显式确认）──

    #[test]
    fn same_name_conflict_is_rejected_and_names_the_source() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        write_file(&root, "grill/SKILL.md", "已经存在的正文");

        let pkg = SkillPackage::from_dir(&skill_dir(home.path(), "grill", "新正文")).unwrap();
        let err = install(&root, &pkg, false).unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("grill"), "报文须含技能名：{msg}");
        assert!(msg.contains("SKILL.md"), "报文须含当前来源：{msg}");

        // 原内容未被改动
        let kept = std::fs::read_to_string(root.join("grill/SKILL.md")).unwrap();
        assert_eq!(kept, "已经存在的正文");
    }

    /// 显式确认后覆盖，且**旧兄弟文件不残留**（整目录换掉）。
    #[test]
    fn overwrite_replaces_the_whole_directory() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        write_file(&root, "grill/SKILL.md", "旧正文");
        write_file(&root, "grill/old-sibling.md", "旧兄弟文件");

        let src = skill_dir(home.path(), "grill", "新正文");
        write_file(&src, "new-sibling.md", "新兄弟文件");
        let pkg = SkillPackage::from_dir(&src).unwrap();
        install(&root, &pkg, true).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("grill/SKILL.md")).unwrap(),
            "新正文"
        );
        assert!(root.join("grill/new-sibling.md").is_file());
        assert!(
            !root.join("grill/old-sibling.md").exists(),
            "覆盖须整目录换掉，不留旧兄弟文件"
        );
    }

    /// 工具型技能（PATH 可执行文件）同名也拦截——它的身份同样占用这个名字。
    #[test]
    fn conflict_with_path_tool_skill_is_also_rejected() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            crate::agent::skills::skill_names(&root).contains(&"sh".to_string()),
            "前提失败：PATH 里没有 sh"
        );
        let src = skill_dir(home.path(), "sh", "想占用 sh 这个名字的正文");
        let pkg = SkillPackage::from_dir(&src).unwrap();
        let err = install(&root, &pkg, false).unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
        assert!(err.to_string().contains("PATH"), "{err}");
    }

    /// **`overwrite = true` 也不能把 PATH 工具型技能「覆盖」掉**：删除只可能作用于技能根里的
    /// 那份，PATH 文件不归本系统管，装了同名 markdown 只会把它遮住——用户以为覆盖了，
    /// 其实那份还在。这类冲突只能改名，故不受 `overwrite` 影响。
    #[test]
    fn overwrite_cannot_shadow_a_path_tool_skill() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            crate::agent::skills::skill_names(&root).contains(&"sh".to_string()),
            "前提失败：PATH 里没有 sh"
        );
        let src = skill_dir(home.path(), "sh", "想遮住 sh 的正文");
        let pkg = SkillPackage::from_dir(&src).unwrap();
        let err = install(&root, &pkg, true).unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
        assert!(
            !root.join("sh").exists(),
            "拒绝后不得留下技能目录：{:?}",
            std::fs::read_dir(&root).map(|d| d.count())
        );
    }

    /// 技能名含 `/` 被拒绝——否则会出现「装得进、列不出、删不掉」的半成品状态
    /// （`discover` 只扫技能根下一层，而 `uninstall` 拒绝含分隔符的名字）。
    #[test]
    fn skill_name_with_separator_is_rejected_consistently() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();

        // 深层嵌套的包：报「层级过深」而不是含糊的「不含 SKILL.md」
        let zip = make_zip(&[("a/b/SKILL.md", "嵌套名字的正文")]);
        let err = SkillPackage::from_zip(&zip, None).unwrap_err();
        assert!(err.to_string().contains("层级过深"), "{err}");
        assert!(!root.join("a").exists(), "拒绝后不得落盘");

        // 显式给一个含分隔符的名字同样拒绝（两个入口同一条不变量）
        let flat = make_zip(&[("SKILL.md", "正文")]);
        let err = SkillPackage::from_zip(&flat, Some("a/b")).unwrap_err();
        assert!(err.to_string().contains("路径分隔符"), "{err}");

        // 名字不变量本身
        assert!(sanitize_skill_name("..").is_err());
        assert!(sanitize_skill_name("a/b").is_err());
        assert!(sanitize_skill_name("grill").is_ok());
    }

    // ── 目录扫描（票 09：名字 + 描述 + 是否已存在）──

    #[test]
    fn scan_lists_skills_with_description_and_existence() {
        let home = tmp();
        let source = home.path().join("zcode-skills");
        let target = home.path().join("skills");
        std::fs::create_dir_all(&target).unwrap();
        write_file(
            &source,
            "grill/SKILL.md",
            "---\ndescription: 拷问设计\n---\n\n正文",
        );
        write_file(&source, "spec/SKILL.md", "没有 frontmatter 的正文");
        // 不是技能：没有 SKILL.md
        write_file(&source, "not-a-skill/readme.md", "杂物");
        // 已经在目标技能根里了
        write_file(&target, "spec/SKILL.md", "已存在");

        let entries = scan_root(&source, Some(&target)).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["grill", "spec"], "杂物不进清单");

        let grill = entries.iter().find(|e| e.name == "grill").unwrap();
        assert_eq!(grill.description.as_deref(), Some("拷问设计"));
        assert!(!grill.exists);
        let spec = entries.iter().find(|e| e.name == "spec").unwrap();
        assert_eq!(spec.description, None);
        assert!(spec.exists, "目标根里已有同名技能须标记 exists");
    }

    #[test]
    fn scan_without_target_root_marks_nothing_as_existing() {
        let home = tmp();
        let source = home.path().join("zcode-skills");
        write_file(&source, "grill/SKILL.md", "正文");
        let entries = scan_root(&source, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].exists);
    }

    #[test]
    fn scan_of_missing_directory_is_an_error() {
        let home = tmp();
        let err = scan_root(&home.path().join("nope"), None).unwrap_err();
        assert!(err.to_string().contains("不可读"), "{err}");
    }

    // ── 批量导入（票 09：逐项成功/失败，不中断整批）──

    #[test]
    fn batch_import_reports_per_item_results() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let source = home.path().join("zcode-skills");
        let good = skill_dir(&source, "good", "好技能");
        let bad = write_file(&source, "bad/readme.md", "没有 SKILL.md");
        let bad_dir = bad.parent().unwrap().to_path_buf();
        let also_good = skill_dir(&source, "also-good", "另一个好技能");

        let results = install_batch(&root, &[good.clone(), bad_dir, also_good], false);
        assert_eq!(results.len(), 3, "逐项结果，一项不落");
        assert!(results[0].1.is_ok(), "{:?}", results[0].1);
        assert!(results[1].1.is_err(), "坏技能须失败：{:?}", results[1].1);
        assert!(
            results[2].1.is_ok(),
            "一个坏技能不得中断整批：{:?}",
            results[2].1
        );
        // 好的两项都真落盘了
        assert!(root.join("good/SKILL.md").is_file());
        assert!(root.join("also-good/SKILL.md").is_file());
    }

    // ── 卸载（票 09）──

    #[test]
    fn uninstall_removes_the_skill_directory() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        write_file(&root, "grill/SKILL.md", "正文");
        write_file(&root, "grill/tests.md", "兄弟文件");
        write_file(&root, "keep/SKILL.md", "另一个技能");

        uninstall(&root, "grill").unwrap();
        assert!(!root.join("grill").exists());
        assert!(root.join("keep/SKILL.md").is_file(), "不得误删别的技能");
        assert!(!crate::agent::skills::skill_names(&root).contains(&"grill".to_string()));
    }

    /// 卸载后引用报错——技能名是唯一身份，不得静默降级（票 09 的核心不变量）。
    ///
    /// 这条断言的是**启动校验**这条路径；`PUT /stage-configs` 走同一份 [`crate::config::validate_startup`]。
    #[test]
    fn uninstalled_skill_breaks_referencing_stage_config() {
        use crate::config::{validate_startup, StartupInputs};
        use crate::types::StageConfig;

        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        write_file(&root, "grill/SKILL.md", "正文");

        let cfg = StageConfig {
            stage: "architect-design".into(),
            skills_json: Some(serde_json::json!(["grill"])),
            ..Default::default()
        };
        let inputs = || StartupInputs {
            settings: Default::default(),
            providers: Vec::new(),
            stage_configs: vec![cfg.clone()],
            available_skills: crate::config::discover_available_skills(&root),
            home_root: Some(home.path().to_path_buf()),
            skills_root: Some(root.clone()),
        };
        assert!(validate_startup(&inputs()).is_ok(), "装着的技能应校验通过");

        uninstall(&root, "grill").unwrap();
        let err = validate_startup(&inputs()).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        assert!(err.to_string().contains("grill"), "{err}");
    }

    #[test]
    fn uninstall_unknown_skill_is_not_found() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let err = uninstall(&root, "no-such-skill").unwrap_err();
        assert!(matches!(err, Error::Task(_)), "API 层映射 404：{err:?}");
    }

    /// 技能根不存在时导入**建根后照装**——`[skills] dir` 覆盖的根有意不被 `ensure_dirs`
    /// 创建（决策 172），若这里也拒绝，用户在无 shell 的界面形态下就装不进第一个技能。
    #[test]
    fn install_creates_the_skills_root_when_missing() {
        let home = tmp();
        let root = home.path().join("not-yet-there/skills");
        assert!(!root.exists(), "前提：技能根尚未存在");

        let pkg =
            SkillPackage::from_zip(&make_zip(&[("first/SKILL.md", "第一个技能")]), None).unwrap();
        install(&root, &pkg, false).unwrap();

        assert!(root.is_dir(), "导入应把技能根建起来");
        assert!(root.join("first/SKILL.md").is_file());
    }

    /// 卸载路径同样不因技能根不存在而 panic（统一报「技能不存在」）。
    #[test]
    fn uninstall_on_missing_root_is_not_found() {
        let home = tmp();
        let root = home.path().join("absent-skills");
        let err = uninstall(&root, "anything").unwrap_err();
        assert!(matches!(err, Error::Task(_)), "{err:?}");
    }

    /// 工具型技能不可卸载（删用户的 PATH 文件是灾难）。
    #[test]
    fn uninstall_refuses_tool_type_skill() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            crate::agent::skills::skill_names(&root).contains(&"sh".to_string()),
            "前提失败：PATH 里没有 sh"
        );
        let err = uninstall(&root, "sh").unwrap_err();
        assert!(err.to_string().contains("工具型"), "{err}");
    }

    /// 卸载路径同样拒绝穿越写法。
    #[test]
    fn uninstall_rejects_traversal_name() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        assert!(uninstall(&root, "../secret").is_err());
        assert!(uninstall(&root, "a/b").is_err());
    }

    // ── 目录导入 ──

    #[test]
    fn directory_import_keeps_siblings_and_nested_files() {
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let src = skill_dir(home.path(), "tdd", "主文档\n\n[tests.md](tests.md)\n");
        write_file(&src, "tests.md", "测试写法");
        write_file(&src, "scripts/run.py", "print('不该被执行')");

        let info = install_from_dir(&root, &src, false).unwrap();
        assert_eq!(info.sibling_count, 2);
        assert!(root.join("tdd/tests.md").is_file());
        assert!(
            root.join("tdd/scripts/run.py").is_file(),
            "嵌套文件须一并落盘"
        );

        // 落盘后兄弟文件展开可用（票 07 的展开依赖文件真的在）
        let body = crate::agent::skills::load_body(&root, "tdd").unwrap();
        assert!(body.contains("测试写法"), "{body}");
    }

    /// 本票不依赖任何网络：全部路径都是本地文件系统（票 09 的最后一条验收项）。
    #[test]
    fn everything_works_offline() {
        // 本模块没有任何网络调用——本用例是这条性质的**可执行断言**：
        // 即使在无网环境，zip 解码（纯 Rust inflate）+ 落盘 + 扫描也全部成立。
        let home = tmp();
        let root = home.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let zip = make_zip(&[("offline/SKILL.md", "离线也能装")]);
        let pkg = SkillPackage::from_zip(&zip, None).unwrap();
        install(&root, &pkg, false).unwrap();
        assert!(root.join("offline/SKILL.md").is_file());
        assert_eq!(scan_root(&root, None).unwrap().len(), 1);
    }
}
