//! 技能来源仓的访问层（决策 194）：从一个 GitHub 仓里**读**技能。
//!
//! 本模块只做三件事——探分支 tip（不下载）、列出仓里所有技能目录、把一个技能目录读成
//! [`SkillPackage`]。**不碰落盘、不碰界面**：落盘仍走票 09 的同一个
//! [`crate::agent::skill_import::install`] 入口（结构校验、同名冲突、路径穿越一处生效）。
//!
//! ## 为什么不是「自定 index registry」（决策 194 的退场决定）
//!
//! 旧形态是 `GET {source}/index.json` 再按其中的 `sha256` 校验下载字节。退场的理由是实测：
//! 生态里 **71 个主机 0 家**发布我们定义的索引，而那 6 家"事实约定"各只发自己的 1 个技能；
//! 且索引里的摘要锚会**漂移**（Pulumi 那个仓的包内容变过而版本号没变）。GitHub 的 commit SHA
//! 是更强的锚——它锚的是**对象哈希**，而 `sha256` 锚的是某一次传输的**字节**。
//!
//! ## 三层身份，别混
//!
//! - **信任单元**是 `owner/repo`（放行一个仓 = 允许从它下载引导 agent 的正文）：[`RepoId`] + [`repo_allowed`]。
//! - **权威身份**是 commit SHA（40 位十六进制）：[`Oid`]，对象哈希由 libgit2 在 fetch 时本地校验。
//! - **落盘名**是技能目录的 basename：[`SkillRef::name`]，落盘前由 `skill_import` 复核。
//!
//! ## URL 只能由我们构造（安全，非洁癖）
//!
//! 形态只有一种：`{base}/{owner}/{repo}.git`，`base` 默认 `https://github.com`。
//! 理由是 libgit2 的传输注册表里 `git://` / `http://` / `https://` / `file://` / `ssh://`
//! **全在**，而且**裸文件系统路径也会被 local transport 吃掉**（`transport_find_fn` 判的是
//! `git_fs_path_exists(url) && is_dir(url)`）。用户在「添加一个仓」里填的那个字符串若直接当
//! URL 用，走哪条 transport 就由它决定。故 [`RepoId::parse`] 拒绝带 scheme / `@` / `..` /
//! 多余斜杠 / 非 ASCII 的输入，URL 由 [`RepoId::url`] 拼。
//!
//! `base` 只有一个测试接缝：环境变量 `AGENTPIPELINE_MARKET_GIT_BASE`（见 [`git_base`]），
//! 取值受与决策 177③ 同一条规则约束（回环放行明文 http，非回环必须 https）。这不放宽安全
//! 口径——**放行的仍是 `owner/repo`，主机仍由我们定**，只是那个主机在测试里可以指回本机回环，
//! 而回环本来就在放行之列。
//!
//! ## 不跟随跨站重定向（决策 177② 的接续）
//!
//! `FetchOptions::new()` 的默认是 `RemoteRedirect::Initial`——它**会**跟随初始请求的跨站重定向，
//! 靠默认值等于当场破掉 177②。故 [`Libgit2Repo`] 显式设 `RemoteRedirect::None`。
//!
//! **但 `None` 的真实语义是「不跟跨站重定向」，不是「密不透风」**：libgit2 对**同站
//! http→https 升级**仍然放行（`src/util/net.c` 里只在目标 scheme 不是 https 时才拒绝跨 scheme
//! 跳转，host 检查则被 `allow_offsite` 关掉）。我们只走 https，这条残余不可达——写在这里
//! 免得后人以为 `None` 挡住了全部跳转。
//!
//! ## 八类失败互不混淆（判据：每类对应一个**互不相同的用户动作**）
//!
//! [`KIND_NETWORK`] 重试 / [`KIND_REPO_NOT_FOUND`] 改仓名 / [`KIND_COMMIT_NOT_FOUND`] 换 commit /
//! [`KIND_SKILL_NOT_FOUND`] 换技能 / [`KIND_REPO_UNREADABLE`] 换仓（或知道本版不支持私有仓）/
//! [`KIND_DIGEST`] 别装、报警 / [`KIND_REPO_NOT_ALLOWED`] 去白名单加它 / [`KIND_TOO_LARGE`] 换更小的仓。
//! 混在一起的代价与票 10 那五类一样：用户不知道该改什么。API 层按 `kind` 分派状态码。
//!
//! ## 明确不做（决策 194）
//!
//! 私有仓（无凭据入口、界面上不放 token 输入框）。可行性已量过：`FetchOptions::custom_headers`
//! 能逐字转发 `Authorization`（实测在 `info/refs` 与 `git-upload-pack` 两跳都到了服务端），
//! 故日后要做是**纯增量**，且凭据可以只从环境变量读而不落盘（不触决策 112 那条
//! 「provider 密钥目前明文存储」）。本版只承担它的报错面。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use git2::{Direction, FetchOptions, ObjectType, RemoteCallbacks, RemoteRedirect, Repository};

use crate::agent::skill_import::{self, SkillPackage};
use crate::agent::skills::{parse_frontmatter, SKILL_FILE};
use crate::error::{Error, Result};

// ─────────────────────────────── 失败类别 ───────────────────────────────

/// 连不上（DNS / 连接被拒 / TLS / 超时 / 非 2xx）。用户该做的是**重试**。
pub const KIND_NETWORK: &str = "market_network";
/// 仓不存在（GitHub 上没这个名字）。用户该做的是**改仓名**。
pub const KIND_REPO_NOT_FOUND: &str = "repo_not_found";
/// 那个 commit 取不到。用户该做的是**改 / 换 commit**。
pub const KIND_COMMIT_NOT_FOUND: &str = "commit_not_found";
/// 这个仓里没有那个技能目录。用户该做的是**换技能**。
pub const KIND_SKILL_NOT_FOUND: &str = "skill_not_found";
/// 仓读不到（无权限 / 私有仓）。用户该做的是**换仓**，或知道本版不支持私有仓。
pub const KIND_REPO_UNREADABLE: &str = "repo_unreadable";
/// 对象哈希不符（libgit2 在 fetch 时本地校验失败）。用户该做的是**别装、报警**。
pub const KIND_DIGEST: &str = "digest_mismatch";
/// 仓不在白名单里。用户该做的是**去界面把这个仓加进白名单**。
pub const KIND_REPO_NOT_ALLOWED: &str = "repo_not_allowed";
/// 传输字节超过上限而中断。用户该做的是**换更小的仓，或改指一个子目录**。
pub const KIND_TOO_LARGE: &str = "download_too_large";

/// 一次取仓允许收到的最大字节数（64 MiB）。
///
/// 与票 09 本地导入端点的 `DefaultBodyLimit` **同值**——票面的意图是「远程包不比本地上传的包
/// 享有更宽的路」，本地那道限制在 HTTP 体上，远程若没有对等的一道，就是一处不对称。
/// 对「一个 markdown 技能目录」有三个数量级余量。
pub const MAX_FETCH_BYTES: usize = 64 * 1024 * 1024;

/// 单个 blob 允许读进内存的上限（16 MiB）。
///
/// 与 `skill_import` 的 `MAX_ENTRY_BYTES` 同值。**权威判定仍在那边**（重打出来的 zip 会再过
/// 一遍 `from_zip` 的逐条目校验），这里这一道只是「别先把 GB 级 blob 读进内存再去判」。
/// 那个常量是私有的，故这里有一份镜像——两处不一致的后果是「报到不同的层」，不是漏判。
const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;

/// 默认的仓基础地址。
pub const DEFAULT_GIT_BASE: &str = "https://github.com";

/// 覆盖仓基础地址的环境变量（**测试接缝**，与 `AGENTPIPELINE_HOME` 同族）。
pub const GIT_BASE_ENV: &str = "AGENTPIPELINE_MARKET_GIT_BASE";

/// 递归深度上限。git 的树不可能有这么深，这条是「别让畸形对象把栈吃光」的兜底。
const MAX_TREE_DEPTH: usize = 32;

// ─────────────────────────────── RepoId ───────────────────────────────

/// 一个**校验过的**仓身份（`owner/repo`）。
///
/// 用类型而不是裸字符串：它是本系统的信任单元（放行一个仓 = 允许从它下载引导 agent 的正文），
/// 而「合法」这条判定必须只有一处实现——配置解析、界面保存、读取层都收在这一个构造器上。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoId {
    pub owner: String,
    pub name: String,
}

impl RepoId {
    /// 解析并归一 `owner/repo`。
    ///
    /// 归一掉的只有**两种无歧义的粘贴残留**：一个 `github.com/` 前缀（用户会直接从浏览器
    /// 地址栏复制）与一个 `.git` 后缀（从 clone 命令里复制）。除此之外**一律拒绝**，不做猜谜：
    ///
    /// - 带 scheme（`https://…`）→ 拒。**这是安全判定，不是洁癖**：URL 由 [`RepoId::url`] 拼，
    ///   若用户输入能带 scheme，走哪条 transport 就由输入决定了（libgit2 认 `git://` / `ssh://` /
    ///   `file://`，甚至裸文件系统路径）。
    /// - 含 `@` → 拒（`user@host` 形态会把 URL 变成带 userinfo 的地址）。
    /// - 含 `..` / 空段 / 多余 `/` → 拒（路径穿越的原料）。
    /// - 非 ASCII 或字符集之外 → 拒（GitHub 的 owner / repo 都是 `[A-Za-z0-9._-]`）。
    ///
    /// **大小写照收不改**：GitHub 认大小写不敏感，但 `Obra/Superpowers` 有展示意义，
    /// 归一里小写会让界面显示出用户没输入过的样子。比较时另按 `eq_ignore_ascii_case`。
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(bad_repo(
                raw,
                "空的仓名：应为 owner/repo（如 obra/superpowers）",
            ));
        }
        // 粘贴残留①：`https://github.com/` / `github.com/` 前缀（大小写不敏感）
        let lower = trimmed.to_ascii_lowercase();
        let without_host = ["https://github.com/", "http://github.com/", "github.com/"]
            .iter()
            .find_map(|prefix| lower.starts_with(prefix).then(|| &trimmed[prefix.len()..]))
            .unwrap_or(trimmed);
        // 粘贴残留②：`.git` 后缀
        let without_git = without_host
            .strip_suffix(".git")
            .or_else(|| without_host.strip_suffix(".GIT"))
            .unwrap_or(without_host);
        let slug = without_git.trim_end_matches('/');

        let mut parts = slug.split('/');
        let (owner, name, extra) = (parts.next(), parts.next(), parts.next());
        let (Some(owner), Some(name)) = (owner, name) else {
            return Err(bad_repo(
                raw,
                "应为恰好一段斜杠分开的 owner/repo（如 obra/superpowers）",
            ));
        };
        if extra.is_some() {
            return Err(bad_repo(raw, "多了一段路径：仓名只到 owner/repo 为止"));
        }
        for (label, part) in [("owner", owner), ("repo", name)] {
            if part.is_empty() {
                return Err(bad_repo(raw, &format!("{label} 是空的")));
            }
            if !part.is_ascii() {
                return Err(bad_repo(raw, &format!("{label} 含非 ASCII 字符：{part}")));
            }
            if part.contains("..") {
                return Err(bad_repo(raw, &format!("{label} 含 `..`：{part}")));
            }
            if part.starts_with('.') || part.starts_with('-') {
                return Err(bad_repo(
                    raw,
                    &format!("{label} 不能以 `.` 或 `-` 开头：{part}"),
                ));
            }
            if !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err(bad_repo(
                    raw,
                    &format!("{label} 含不合法字符（只允许字母数字与 `-` `_` `.`）：{part}"),
                ));
            }
        }
        Ok(RepoId {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// 归一形态 `owner/repo`（界面与错误报文用这个）。
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// 克隆地址。**形态只有这一种**，由本函数拼——见类型文档里那条安全理由。
    pub fn url(&self, base: &str) -> String {
        format!(
            "{}/{}/{}.git",
            base.trim_end_matches('/'),
            self.owner,
            self.name
        )
    }
}

fn bad_repo(raw: &str, why: &str) -> Error {
    Error::Market {
        kind: KIND_REPO_NOT_ALLOWED.into(),
        message: format!("仓名不合法：{why}（收到的是「{raw}」）"),
        raw: format!("repo = {raw}"),
    }
}

// ─────────────────────────────── Oid ───────────────────────────────

/// 一个**校验过的**完整 commit SHA（40 位小写十六进制）。
///
/// 为什么不做成裸字符串：实测（票 01 变体 I）**7 位缩写 SHA 会让 `fetch` 返回 `Ok` 但什么都不取**
/// ——0.74 s、「成功」、无 ref、无对象、无 `shallow` 文件、**无任何错误**。不校验就会报「装好了」
/// 而其实没装。同一族还有第二个静默空转：refspec 指向一个不存在的 ref 时同样 `Ok` 而无所得。
/// 故本类型 + [`Libgit2Repo`] 取完之后的 `find_commit` 复验，是两道独立的判定。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Oid(String);

impl Oid {
    /// 解析一个完整 SHA（小写归一；大小写混写的十六进制照收）。
    ///
    /// 形态不合法时归到 [`KIND_COMMIT_NOT_FOUND`]：用户的动作与「这个 SHA 取不到」完全相同
    /// （换一个 commit），不为它单造第九类。
    pub fn parse(raw: &str) -> Result<Self> {
        let value = raw.trim().to_ascii_lowercase();
        if value.len() != 40 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Market {
                kind: KIND_COMMIT_NOT_FOUND.into(),
                message: format!(
                    "commit 必须是完整的 40 位十六进制 SHA（收到的是「{}」，{} 位）。\
                     缩写 SHA 会让 git 返回成功却什么都没取到——请用列表上给出的那个完整 SHA",
                    raw.trim(),
                    raw.trim().len()
                ),
                raw: format!("commit = {raw}"),
            });
        }
        Ok(Oid(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 前 7 位（界面显示用）。
    pub fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl std::fmt::Display for Oid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ─────────────────────────────── 扫描层 ───────────────────────────────

/// 仓里的一个技能目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRef {
    /// 仓根相对路径（`skills/grill`；仓根本身就是技能时是空串）。
    pub dir: String,
    /// 技能名 = `dir` 的 basename（仓根技能取仓名）。**扫描层的唯一判据**。
    pub name: String,
    /// `SKILL.md` frontmatter 的 `description`（缺省 `None`；读不出来也 `None`）。
    pub description: Option<String>,
}

// 原 `pub trait SkillRepo` 已删（决策 250）：全仓只有一处 `impl`，测试真正的替换原语
// 是 [`Libgit2Repo::with_base`] 与 `AGENTPIPELINE_MARKET_GIT_BASE`（远端地址——生产对
// GitHub、测试对离线 fixture 服务），trait 从未提供过第二种 adapter，是假想 seam。
// 三个读方法现为 [`Libgit2Repo`] 的固有方法，语义与签名逐字不变。

// ─────────────────────────────── 纯函数 ───────────────────────────────

/// 仓是否放行。
///
/// **这是本系统唯一的安全控制**（决策 187 的原话是「这条判定不能有第二个版本」，决策 194 把
/// 判定对象从 origin 换成 `owner/repo`）。故照 [`crate::agent::egress::check_allow_host`] 的姿态
/// 做成**纯函数**，配自己的单测——而不是散在端点里。
///
/// 空白名单 = **不放行任何仓**（保守方向上的默认：忘配的代价是装不上，用户立刻发现；
/// 配宽的代价是静默从陌生仓装上引导 agent 的正文）。
///
/// 比较按 `eq_ignore_ascii_case`：GitHub 认大小写不敏感，而界面与配置两处的大小写不保证一致。
pub fn repo_allowed(repo: &RepoId, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() {
        return Err(Error::Market {
            kind: KIND_REPO_NOT_ALLOWED.into(),
            message: format!(
                "未放行任何技能来源仓，已拒绝从 {} 安装：请在「技能市场」页把 {}/{} 加进仓名单\
                 （默认空 = 不从任何仓安装；本地导入不受影响）",
                repo.slug(),
                repo.owner,
                repo.name
            ),
            raw: format!("requested = {}, allowed = []", repo.slug()),
        });
    }
    if allowed
        .iter()
        .filter_map(|raw| RepoId::parse(raw).ok())
        .any(|a| a.slug().eq_ignore_ascii_case(&repo.slug()))
    {
        return Ok(());
    }
    Err(Error::Market {
        kind: KIND_REPO_NOT_ALLOWED.into(),
        message: format!(
            "技能来源仓未放行：{}（请在「技能市场」页把它加进仓名单——\
             放行一个仓 = 允许从它下载引导 agent 的正文）",
            repo.slug()
        ),
        raw: format!("requested = {}, allowed = {allowed:?}", repo.slug()),
    })
}

/// 当前生效的仓基础地址（`AGENTPIPELINE_MARKET_GIT_BASE` 优先，否则 [`DEFAULT_GIT_BASE`]）。
///
/// **非法的覆盖值回落默认而不是报错**：它是测试接缝，不是用户配置项——一个畸形环境变量
/// 若让整个技能市场不可用，那是把测试设施变成了生产故障源。真正的用户可配面是仓名单。
pub fn git_base() -> String {
    match std::env::var(GIT_BASE_ENV) {
        Ok(raw) => match normalize_git_base(&raw) {
            Ok(base) => base,
            Err(why) => {
                tracing::warn!(
                    env = GIT_BASE_ENV,
                    value = %raw,
                    reason = %why,
                    "仓基础地址的覆盖值非法，回落默认值"
                );
                DEFAULT_GIT_BASE.to_string()
            }
        },
        Err(_) => DEFAULT_GIT_BASE.to_string(),
    }
}

/// 校验并归一一个仓基础地址。
///
/// 与决策 177③ 同一条规则：**非回环必须 https**（明文 http 上中间人可以替换整份技能正文），
/// 回环（`127.0.0.0/8` / `localhost` / `::1`）放行明文 http，让本机起一个 smart HTTP fixture
/// 做开发与测试不必自签证书。
///
/// 只到主机（可带端口）：带路径 / 查询 / 片段的值会让「仓 URL 的形态唯一」这条不再成立。
pub fn normalize_git_base(raw: &str) -> std::result::Result<String, String> {
    let value = raw.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("基础地址是空的".into());
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err("基础地址必须带 scheme（https://… 或回环的 http://…）".into());
    };
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err(format!("只支持 http / https，收到的是 {scheme}"));
    }
    if rest.is_empty() {
        return Err("基础地址没有主机名".into());
    }
    if rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err("基础地址只能到主机（可带端口），不带路径 / 查询 / 片段".into());
    }
    if rest.contains('@') || rest.chars().any(char::is_whitespace) {
        return Err("基础地址的主机名不合法".into());
    }
    let host = if let Some(v6) = rest.strip_prefix('[') {
        v6.split(']').next().unwrap_or("").to_string()
    } else {
        rest.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        return Err("基础地址没有主机名".into());
    }
    // 判据走 host_policy（决策 246 的唯一实现）——原先是本文件自带的一份 `IpAddr` 谓词，
    // 其「前缀伪装不算回环」断言已迁入 `host_policy` 的共享表测试。
    if scheme == "http" && !crate::host_policy::is_loopback(&host) {
        return Err(format!(
            "非回环的主机必须用 https：{value}。明文 http 挡不住中间人——\
             攻击者可以替换整份技能正文（决策 177③）"
        ));
    }
    Ok(format!("{scheme}://{rest}"))
}

/// 从广告的 ref 列表里挑出「默认分支的 tip」。
///
/// 顺序刻意如此：**先认 `default_branch()` 给的 symref 目标**（真服务器都会给），
/// 认不出再回落到 `main` → `master` → 第一个 `refs/heads/*`。
/// 做成纯函数是为了能脱开网络钉住这段挑选逻辑——它是 [`Libgit2Repo::head`] 唯一有分支的地方。
pub fn pick_default_tip<'a>(
    heads: &'a [(String, Oid)],
    default_branch: Option<&str>,
) -> Option<&'a Oid> {
    let by_name = |want: &str| heads.iter().find(|(name, _)| name == want);
    if let Some(target) = default_branch {
        if let Some((_, oid)) = by_name(target) {
            return Some(oid);
        }
    }
    for fallback in ["refs/heads/main", "refs/heads/master"] {
        if let Some((_, oid)) = by_name(fallback) {
            return Some(oid);
        }
    }
    heads
        .iter()
        .find(|(name, _)| name.starts_with("refs/heads/"))
        .map(|(_, oid)| oid)
}

/// 校验一个技能目录路径（仓根内相对路径）并归一。
///
/// 与 `skill_import::sanitize_rel_path`（zip 条目名那道）是**不同的层**：那道管的是包内的
/// 条目名，这道管的是「树里要走进哪个目录」。两者都拒绝 `..` / 绝对路径 / 空段，理由是同一个
/// （别让输入决定我们读哪里），但**不复用对方的实现**——它们校验的是两种不同的东西。
///
/// 空串是**合法**的：仓根有 `SKILL.md` 时，那个仓本身就是一个技能。
pub fn sanitize_dir(raw: &str) -> std::result::Result<String, String> {
    let unified = raw.replace('\\', "/");
    let trimmed = unified.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains('\0') {
        return Err("含 NUL 字节".into());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err("含 `..` 穿越".into()),
            p if p.ends_with(':') => return Err("含盘符前缀".into()),
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Ok(String::new());
    }
    Ok(parts.join("/"))
}

// ─────────────────────────── 生产实现 Libgit2Repo ───────────────────────────

/// 用 libgit2 从 GitHub 取仓。
///
/// ## 取法固定 `depth(1)`
///
/// 整仓历史对「读几个技能目录」毫无用处。实测 `depth(1)` 1.9–2.3 s、`.git` 212 KiB、
/// `.git/shallow` 落盘且内容就是被取的那个 commit；树与 blob 完整，只有历史被切。
///
/// **`HEAD` 不会被动**（实测）：`Remote::fetch` 不创建也不移动 `HEAD`（它保持 unborn），
/// 取到的 commit 只能经裸 oid 走到（`git fsck` 会报它 dangling）。故我们从不用 `HEAD`——
/// 所有读取都以显式 `find_commit(oid)` 为起点。
///
/// ## 缓存按 (仓, commit)
///
/// 同一个 (仓, commit) 的「列技能」与「读技能」共用一份已 fetch 的裸仓目录——这是界面
/// 「列表钉住浏览那一刻的 commit」能成立的前提（列表与随后的安装不重复拉，也不会中途漂移）。
/// 缓存键是 `{owner}__{repo}__{sha}`，落在 `root` 下。
///
/// ## 取仓是串行的
///
/// 一把锁罩住整个 `fetch`：取仓是「人点一下」的频度，而串行换来的是同一个 `(仓, commit)`
/// 目录不会被两个请求同时写——缓存因此不需要「临时目录 + 原子重命名」那一套。
/// 锁是 `std::sync::Mutex`（取仓发生在 `spawn_blocking` 的阻塞线程上，不是在 async 上下文里）。
///
/// ## 内部是 `Arc`
///
/// 那几个方法返回 `BoxFuture<'static>`，所以 future 里不能借用 `&self`——把状态放进
/// `Arc<Inner>`，每次调用克隆一个 `Arc` 进 future。这也是为什么它可以被廉价地 clone 与共享。
#[derive(Clone)]
pub struct Libgit2Repo {
    inner: Arc<Inner>,
}

struct Inner {
    base: String,
    root: PathBuf,
    max_bytes: usize,
    /// `{owner}__{repo}__{sha}` → 已 fetch 的裸仓目录。
    cache: Mutex<HashMap<String, PathBuf>>,
    /// 取仓的串行闸（见类型文档）。
    fetch_lock: Mutex<()>,
}

impl Libgit2Repo {
    /// 以 `root` 为缓存根构造（生产传 `home.root()` 下的一个私有目录）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Libgit2Repo {
            inner: Arc::new(Inner {
                base: git_base(),
                root: root.into(),
                max_bytes: MAX_FETCH_BYTES,
                cache: Mutex::new(HashMap::new()),
                fetch_lock: Mutex::new(()),
            }),
        }
    }

    /// 指定基础地址（**测试接缝**；生产走 [`git_base`]）。
    ///
    /// 只在构造阶段用（`Arc::get_mut` 拿不到独占引用时静默不动——那种用法本来就是错的，
    /// 但不该为此 panic 掉一个正在服务的工作线程）。
    pub fn with_base(mut self, base: &str) -> Result<Self> {
        let base = normalize_git_base(base)
            .map_err(|why| Error::Config(format!("仓基础地址不合法（{base}）：{why}")))?;
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.base = base;
        }
        Ok(self)
    }

    /// 指定字节上限（**测试接缝**：上限的用例不可能真去下 64 MiB）。
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.max_bytes = max_bytes;
        }
        self
    }

    /// 当前基础地址。
    pub fn base(&self) -> &str {
        &self.inner.base
    }

    /// 缓存目录名：`{owner}__{repo}__{sha}`。
    fn cache_key(repo: &RepoId, commit: &Oid) -> String {
        format!("{}__{}__{}", repo.owner, repo.name, commit.as_str())
    }

    /// 探默认分支 tip 用的空裸仓（**复用同一个**：`connect` + `list` 不写对象、也不建 ref）。
    fn probe_dir(&self) -> PathBuf {
        self.inner.root.join("probe.git")
    }

    fn cache_dir(&self, key: &str) -> PathBuf {
        self.inner.root.join(key)
    }
}

/// 兜底实现：给 `AppState::new` 一个非 `Option` 的初值（契约测试不注入时用的就是它）。
///
/// **生产必须走 `AppState::with_repo`**（`serve.rs` 注的是 `{home}/market-repos`）：
/// 取下来的裸仓要跨"列表"与"安装"两次请求复用，落在系统临时目录里会被清理器顺手删掉，
/// 表现为「刚列出来的 commit 忽然取不到」。
impl Default for Libgit2Repo {
    fn default() -> Self {
        Libgit2Repo::new(std::env::temp_dir().join("agentpipeline-market-repos"))
    }
}

/// 把阻塞的 git2 调用挪出 async 运行时（与 `crate::git` 的 `blocking` 同一姿态）。
async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(inner) => inner,
        Err(e) => Err(market_error(
            KIND_NETWORK,
            format!("取仓任务未能执行：{e}"),
            format!("spawn_blocking: {e}"),
        )),
    }
}

impl Libgit2Repo {
    /// 分支 tip（只 ls-remote，**不下载 pack**）。
    pub fn head(&self, repo: &RepoId) -> BoxFuture<'static, Result<Oid>> {
        let repo = repo.clone();
        let url = repo.url(&self.inner.base);
        let probe = self.probe_dir();
        Box::pin(async move {
            blocking(move || {
                std::fs::create_dir_all(&probe).map_err(|e| fs_error(&probe, e))?;
                let handle = Repository::init_bare(&probe).map_err(|e| {
                    market_error(
                        KIND_NETWORK,
                        format!("准备取仓用的空库失败：{e}"),
                        format!("init_bare {}: {e}", probe.display()),
                    )
                })?;
                // `connect` + `list` 只走握手，**不传任何字节**（实测：4 个 ref，目标 .git 20 KB，
                // objects/pack 是空目录）。这是 head() 与 fetch 的分界线，别用 fetch 代替。
                let (picked, detail) = {
                    let mut remote = handle.remote_anonymous(&url).map_err(|e| {
                        classify_transport_error("构造匿名远端失败", &repo, &url, e)
                    })?;
                    remote
                        .connect(Direction::Fetch)
                        .map_err(|e| classify_transport_error("连接仓失败", &repo, &url, e))?;
                    let default_branch = remote
                        .default_branch()
                        .ok()
                        .and_then(|buf| buf.as_str().map(str::to_string));
                    let heads: Vec<(String, Oid)> = remote
                        .list()
                        .map_err(|e| classify_transport_error("读取 ref 列表失败", &repo, &url, e))?
                        .iter()
                        .filter_map(|head| {
                            let name = head.name().to_string();
                            let oid = Oid::parse(&head.oid().to_string()).ok()?;
                            Some((name, oid))
                        })
                        .collect();
                    let picked = pick_default_tip(&heads, default_branch.as_deref()).cloned();
                    let detail = format!(
                        "refs = {:?}, default = {default_branch:?}",
                        heads.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
                    );
                    (picked, detail)
                    // `remote` 借用了 `handle`，在这里 drop 掉才能让 handle 继续被用
                };
                picked.ok_or_else(|| {
                    market_error(
                        KIND_COMMIT_NOT_FOUND,
                        format!(
                            "仓 {} 里没有任何分支——它可能是刚建的空仓，或默认分支还没推上去",
                            repo.slug()
                        ),
                        detail,
                    )
                })
            })
            .await
        })
    }

    /// 该 commit 下所有技能目录（含 `SKILL.md` 的目录即技能，与深度无关）。
    pub fn list_skills(
        &self,
        repo: &RepoId,
        commit: &Oid,
    ) -> BoxFuture<'static, Result<Vec<SkillRef>>> {
        let me = self.clone();
        let repo = repo.clone();
        let commit = commit.clone();
        let cache = self.cache_dir(&Self::cache_key(&repo, &commit));
        Box::pin(async move {
            blocking(move || {
                let repo_dir = me.ensure_fetched(&repo, &commit, &cache)?;
                let handle = Repository::open(&repo_dir).map_err(|e| open_error(&repo_dir, e))?;
                let commit_obj = find_commit(&handle, &commit, &repo)?;
                let tree = commit_obj
                    .tree()
                    .map_err(|e| tree_unreadable(&repo, &commit, e))?;
                let mut out: Vec<SkillRef> = Vec::new();
                walk_for_skills(&handle, &tree, "", &repo.name, 0, &mut out)?;
                out.sort_by(|a, b| a.dir.cmp(&b.dir));
                Ok(out)
            })
            .await
        })
    }

    /// 读一个技能目录（含子树）成一个 [`SkillPackage`]。
    pub fn read_skill(
        &self,
        repo: &RepoId,
        commit: &Oid,
        dir: &str,
    ) -> BoxFuture<'static, Result<SkillPackage>> {
        let me = self.clone();
        let repo = repo.clone();
        let commit = commit.clone();
        let raw_dir = dir.to_string();
        let cache = self.cache_dir(&Self::cache_key(&repo, &commit));
        Box::pin(async move {
            blocking(move || {
                let skill_dir = sanitize_dir(&raw_dir).map_err(|why| {
                    market_error(
                        KIND_SKILL_NOT_FOUND,
                        format!("技能目录路径不合法（{why}）：{raw_dir}"),
                        format!("dir = {raw_dir}"),
                    )
                })?;
                let repo_dir = me.ensure_fetched(&repo, &commit, &cache)?;
                let handle = Repository::open(&repo_dir).map_err(|e| open_error(&repo_dir, e))?;
                let commit_obj = find_commit(&handle, &commit, &repo)?;
                let tree = commit_obj
                    .tree()
                    .map_err(|e| tree_unreadable(&repo, &commit, e))?;

                // 仓根技能：`dir` 是空串，整棵树就是技能
                let sub = if skill_dir.is_empty() {
                    tree
                } else {
                    let entry = tree
                        .get_path(Path::new(&skill_dir))
                        .map_err(|_| skill_not_found(&repo, &commit, &skill_dir))?;
                    if entry.kind() != Some(ObjectType::Tree) {
                        return Err(skill_not_found(&repo, &commit, &skill_dir));
                    }
                    handle
                        .find_tree(entry.id())
                        .map_err(|_| skill_not_found(&repo, &commit, &skill_dir))?
                };

                let name = if skill_dir.is_empty() {
                    repo.name.clone()
                } else {
                    skill_dir
                        .rsplit('/')
                        .next()
                        .unwrap_or(&repo.name)
                        .to_string()
                };

                let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
                collect_files(&handle, &sub, "", 0, &mut files)?;
                if !files.contains_key(SKILL_FILE) {
                    return Err(skill_not_found(&repo, &commit, &skill_dir));
                }

                // 名字先过引擎那道判定（`check_skill_name`，与本地导入同一条不变量），
                // **再**重打：否则一个非法目录名会先被重打成 `../evil/SKILL.md` 这种条目名，
                // 报出来的是「zip 条目名非法（含 `..` 穿越）」——用户看不出问题在技能名上。
                let name = skill_import::check_skill_name(&name).map_err(|why| {
                    market_error(
                        KIND_SKILL_NOT_FOUND,
                        format!("这个技能的目录名不能用作落盘目录名（{why}）：{name}"),
                        format!("dir = {skill_dir}, name = {name}"),
                    )
                })?;

                // 重打成 `{name}/…` 的**单根 zip** 再交给既有的 `from_zip`——不在引擎里新开
                // 一个构造器。理由：这样 `sanitize_rel_path`、`enclosed_name` 那道独立复检、
                // `MAX_ENTRIES` / `MAX_ENTRY_BYTES`、`validate_single_root`、frontmatter 校验
                // **每一道都留在路径上**。新增构造器会绕过其中几道，等于为远程来源开了一条
                // 比本地上传更短的路——而「远程包不比本地上传的包享有更宽的路」是本系统明写的口径。
                // 技能是 markdown，几十 KB 的内存 zip 往返不构成成本。
                let zip = repack(&name, &files)?;
                SkillPackage::from_zip(&zip, Some(&name))
            })
            .await
        })
    }
}

impl Libgit2Repo {
    /// 保证 `(仓, commit)` 已在本地，返回那个裸仓目录。
    ///
    /// 串行（见类型文档），且**取完必须复验对象真的在**——这是挡那两条静默空转
    /// （缩写 SHA / 不存在的 refspec）的第二道判定：`fetch` 返回 `Ok` 不等于取到了东西。
    fn ensure_fetched(&self, repo: &RepoId, commit: &Oid, dir: &Path) -> Result<PathBuf> {
        let key = Self::cache_key(repo, commit);
        if let Some(hit) = self.cache_get(&key) {
            return Ok(hit);
        }
        // 取仓期间不放锁：这是本模块唯一会阻塞在网络上的地方
        let _serial = self.inner.fetch_lock.lock().map_err(|_| {
            market_error(
                KIND_NETWORK,
                "取仓闸锁中毒（前一次取仓崩了）".into(),
                "fetch_lock poisoned".into(),
            )
        })?;
        // 拿到闸之后再查一次：等锁期间别人可能已经取完了
        if let Some(hit) = self.cache_get(&key) {
            return Ok(hit);
        }

        let url = repo.url(&self.inner.base);
        std::fs::create_dir_all(dir).map_err(|e| fs_error(dir, e))?;
        let handle = Repository::init_bare(dir).map_err(|e| {
            market_error(
                KIND_NETWORK,
                format!("准备取仓用的空库失败：{e}"),
                format!("init_bare {}: {e}", dir.display()),
            )
        })?;

        let latest = Arc::new(AtomicUsize::new(0));
        let aborted = Arc::new(AtomicBool::new(false));
        let cap = self.inner.max_bytes;
        {
            let mut remote = handle
                .remote_anonymous(&url)
                .map_err(|e| classify_transport_error("构造匿名远端失败", repo, &url, e))?;
            let mut callbacks = RemoteCallbacks::new();
            {
                let latest = Arc::clone(&latest);
                let aborted = Arc::clone(&aborted);
                callbacks.transfer_progress(move |progress| {
                    // `received_bytes` 是「到此刻为止收到的 packfile 字节」。返回 `false` 会中止
                    // fetch——这是**边收边判**，不是下载前的门：回调只在读块粒度上被叫
                    // （实测最小约 64 KB），故超限只能发生在「一块已经落下来之后」。
                    // 被中断的目的地留下的是一个合法但空的仓（无 indexer 临时 pack、无 ref），
                    // 不需要额外清理。
                    let total = progress.received_bytes();
                    latest.store(total, Ordering::SeqCst);
                    if total > cap {
                        aborted.store(true, Ordering::SeqCst);
                        return false;
                    }
                    true
                });
            }
            let mut opts = FetchOptions::new();
            opts.remote_callbacks(callbacks)
                .depth(1)
                // **必须显式设**：默认是 `Initial`（跟初始请求的跨站重定向），靠默认值等于
                // 当场破掉决策 177②。注意它的真实语义只是「不跟跨站」——同站 http→https 升级
                // 仍放行，而我们只走 https，故那条残余不可达。
                .follow_redirects(RemoteRedirect::None);
            // refspec 就是一个裸 SHA（没有 `:目标`），libgit2 把它当成 want 而不更新任何 ref：
            // 我们要的正是「取到对象」，不是「建一个分支」。`depth(1)` 时本地 `HEAD` 保持 unborn。
            if let Err(err) = remote.fetch(&[commit.as_str()], Some(&mut opts), None) {
                if aborted.load(Ordering::SeqCst) {
                    // 中断有两种错误形态（回调返回 false → `indexer progress callback returned -1`；
                    // 走 GIT_EUSER 的那条 → `class=None code=User msg=no error`，**报文里什么都没有**）。
                    // 故数字只能来自我们自己记的这份，不能来自错误串。
                    return Err(too_large(
                        repo,
                        commit,
                        latest.load(Ordering::SeqCst),
                        cap,
                        &err.to_string(),
                    ));
                }
                // **顺序要紧**：先认「对象哈希不符」——那是"别装、报警"，与"换一个 commit"
                // 是两件完全不同的事，而它同样会让 `find_commit` 找不到对象（对象没落地）。
                if is_digest_shaped(&err) {
                    return Err(digest_mismatch(repo, commit, &err));
                }
                // 协议层面被拒（对象到不了手）与传输层失败**看起来是同一件事**——libgit2 把
                // 服务端那句 `ERR … not our ref` 丢掉了（见 [`commit_unavailable`]）。区别它们
                // 的唯一办法是问**本地对象库**：取完之后对象确实不在，那就是「这个 commit 取不到」。
                if !is_transport_shaped(&err) && find_commit(&handle, commit, repo).is_err() {
                    return Err(commit_unavailable(repo, commit, &err));
                }
                return Err(classify_fetch_error(repo, &url, err));
            }
        }
        if aborted.load(Ordering::SeqCst) {
            // 中断发生在 fetch 的收尾路径上时，错误可能被吞成 `Ok`
            return Err(too_large(
                repo,
                commit,
                latest.load(Ordering::SeqCst),
                cap,
                "回调在收尾阶段中止",
            ));
        }

        // 复验：`Ok` 不等于取到了（见 [`Oid`] 的文档）。对象在本地对象库里就一定能 find 到。
        find_commit(&handle, commit, repo)?;

        if let Ok(mut cache) = self.inner.cache.lock() {
            cache.insert(key, dir.to_path_buf());
        }
        Ok(dir.to_path_buf())
    }

    fn cache_get(&self, key: &str) -> Option<PathBuf> {
        self.inner
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(key).cloned())
    }
}

// ─────────────────────────── 错误构造与分类 ───────────────────────────

fn market_error(kind: &str, message: String, raw: String) -> Error {
    Error::Market {
        kind: kind.into(),
        message,
        raw,
    }
}

fn fs_error(path: &Path, err: std::io::Error) -> Error {
    market_error(
        KIND_NETWORK,
        format!("取仓的本地目录不可用（{}）：{err}", path.display()),
        format!("{}: {err}", path.display()),
    )
}

fn tree_unreadable(repo: &RepoId, commit: &Oid, err: git2::Error) -> Error {
    market_error(
        KIND_COMMIT_NOT_FOUND,
        format!(
            "读不到仓 {} 在 {} 的目录树：这个 commit 可能是个不完整的对象",
            repo.slug(),
            commit.short()
        ),
        format!("tree of {}: {err}", commit.as_str()),
    )
}

fn skill_not_found(repo: &RepoId, commit: &Oid, dir: &str) -> Error {
    let where_ = if dir.is_empty() {
        "仓根".to_string()
    } else {
        dir.to_string()
    };
    market_error(
        KIND_SKILL_NOT_FOUND,
        format!(
            "仓 {} 在 {} 里找不到这个技能：{where_}（技能就是「含 {SKILL_FILE} 的目录」）。\
             换一个技能，或点刷新重新列一次",
            repo.slug(),
            commit.short()
        ),
        format!(
            "repo = {}, commit = {}, dir = {dir}",
            repo.slug(),
            commit.as_str()
        ),
    )
}

fn too_large(repo: &RepoId, commit: &Oid, seen: usize, cap: usize, why: &str) -> Error {
    market_error(
        KIND_TOO_LARGE,
        format!(
            "仓 {}（{}）超过 {cap} 字节上限，已中断取仓：**已收到约 {seen} 字节**。\
             技能是几十 KB 量级的 markdown，这个体积不正常——请换一个更小的仓，\
             或改指一个子目录",
            repo.slug(),
            commit.short()
        ),
        format!("received = {seen}, cap = {cap}, detail = {why}"),
    )
}

fn find_commit<'a>(
    handle: &'a Repository,
    commit: &Oid,
    repo: &RepoId,
) -> Result<git2::Commit<'a>> {
    let oid = git2::Oid::from_str(commit.as_str()).map_err(|e| {
        market_error(
            KIND_COMMIT_NOT_FOUND,
            format!("commit 不是合法的对象 id：{}", commit.as_str()),
            format!("{e}"),
        )
    })?;
    handle.find_commit(oid).map_err(|e| {
        market_error(
            KIND_COMMIT_NOT_FOUND,
            format!(
                "仓 {} 里取不到 commit {}：这个 SHA 在它的历史里不存在，或对象没能落地。\
                 请点刷新重新列一次，再按列表上的 SHA 安装",
                repo.slug(),
                commit.short()
            ),
            format!(
                "repo = {}, commit = {}, git2: {e}",
                repo.slug(),
                commit.as_str()
            ),
        )
    })
}

/// 连接 / 列 ref 阶段的错误分类（还没进入「取对象」）。
fn classify_transport_error(context: &str, repo: &RepoId, url: &str, err: git2::Error) -> Error {
    let text = err.message().to_ascii_lowercase();
    if is_auth_shaped(err.code(), &text) {
        return unreadable(context, repo, &err);
    }
    if is_not_found_shaped(err.code(), &text) {
        return not_found(context, repo, &err);
    }
    market_error(
        KIND_NETWORK,
        format!(
            "取仓失败（连不上 GitHub）：{context}。请检查网络是否可达；\
             本机无网时本地导入不受影响"
        ),
        format!("{context}：{url} {}", err.message()),
    )
}

/// 取对象阶段的错误分类（`fetch` 返回 Err）。
fn classify_fetch_error(repo: &RepoId, url: &str, err: git2::Error) -> Error {
    let text = err.message().to_ascii_lowercase();
    if is_auth_shaped(err.code(), &text) {
        return unreadable("取仓", repo, &err);
    }
    if is_not_found_shaped(err.code(), &text) {
        return not_found("取仓", repo, &err);
    }
    market_error(
        KIND_NETWORK,
        format!(
            "取仓失败：与 GitHub（{url}）的连接中断或超时。请稍后重试；\
             本机无网时本地导入不受影响"
        ),
        format!(
            "fetch：{} code={:?} class={:?} msg={}",
            repo.slug(),
            err.code(),
            err.class(),
            err.message()
        ),
    )
}

/// 这个 commit 到不了手上——**实测**得来的分类，别改回按报文匹配。
///
/// 实测（2026-09-16，真 GitHub）：对一个不存在的 `want`，GitHub 回的是 **HTTP 200** +
/// 一条 pkt-line `0049ERR upload-pack: not our ref <sha>`（`git http-backend` 不看
/// `upload-pack` 的退出码，只把 stdout 流回去；本仓的离线 fixture 已按同一形态改齐）。
/// 而 **libgit2 把那句话丢掉了**：交到我们手上只剩 `code=GenericError class=Net
/// msg=unexpected packet type`——按报文匹配等于把分类挂在 libgit2 的措辞上，换一个版本就飘；
/// 按 `ErrorClass::Net` 分也不行，真连不上用的**就是**这个 class（见 [`is_transport_shaped`]）。
///
/// 故判据是**本地对象库**：取完之后对象确实不在，就是「这个 commit 取不到」——
/// 用户该做的是换一个 commit，而不是重试网络。[`is_transport_shaped`] 是它的前置：
/// 真连不上 / 真 404 的走各自那几类，不会掉进这里。
fn commit_unavailable(repo: &RepoId, commit: &Oid, err: &git2::Error) -> Error {
    market_error(
        KIND_COMMIT_NOT_FOUND,
        format!(
            "仓 {} 里取不到 commit {}：它可能不在这个仓的历史里，或在对方 force-push 之后\
             已经不存在。请在「技能市场」页重新浏览该仓、按刷新后的短 SHA 再装",
            repo.slug(),
            commit.short()
        ),
        format!(
            "fetch：{} {} class={:?} code={:?} msg={}",
            repo.slug(),
            commit.short(),
            err.class(),
            err.code(),
            err.message()
        ),
    )
}

/// 对象哈希不符——**libgit2 在解析 pack / 校验对象时本地发现**。
///
/// 它比"下载字节的 sha256"更强：查的是 git 的对象哈希，覆盖目录结构（决策 194 裁决③）。
/// 用户该做的是**别装、报警**（中间人、传输损坏、或对面给了个坏包），与"换一个 commit"
/// 是两回事，故必须与 [`commit_unavailable`] 分开判——两者都会让 `find_commit` 找不到对象。
///
/// **本仓的离线 fixture 造不出这个失败**（要手搓一个哈希坏掉的 pack），故它**没有端到端用例**；
/// 这条判定由下面的单测钉住措辞与 class。诚实记账见票 02 的实施记录。
fn is_digest_shaped(err: &git2::Error) -> bool {
    if matches!(err.class(), git2::ErrorClass::Sha1) {
        return true;
    }
    let text = err.message().to_ascii_lowercase();
    ["hash mismatch", "checksum", "corrupt", "invalid object"]
        .iter()
        .any(|phrase| text.contains(phrase))
}

/// 哈希不符的报文：说清"这不是网络问题、也不是你填错了"，并劝住"别装"。
fn digest_mismatch(repo: &RepoId, commit: &Oid, err: &git2::Error) -> Error {
    market_error(
        KIND_DIGEST,
        format!(
            "从仓 {} 取 {} 时，**对象哈希对不上**：收到的内容与 git 自己算出来的对不上号。\
             可能是传输损坏，也可能有人在中途改过内容——**建议先别装**，重试一次仍如此就换一个来源仓",
            repo.slug(),
            commit.short()
        ),
        format!(
            "fetch：{} {} class={:?} code={:?} msg={}",
            repo.slug(),
            commit.short(),
            err.class(),
            err.code(),
            err.message()
        ),
    )
}

/// 这次失败是不是**传输层**的（连接 / TLS / HTTP 状态 / 认证）。
///
/// 它存在的唯一理由是给 [`commit_unavailable`] 划边界。**别用 `ErrorClass` 当判据**：
/// 实测协议层被拒（对象到不了手）报出来的 class 也是 [`git2::ErrorClass::Net`]，
/// 与真连不上**同形**——按它分，`commit_not_found` 这一类永远打不到。
/// 真正能分开的是两件事：① 传输层那几个"见过"的措辞（本模块自己的用例天天在产它们：
/// `error receiving data from socket`、`Broken pipe`、`Connection reset by peer`、
/// `unexpected EOF`）；② HTTP 状态与 TLS 这两类由 class 独占。
/// 「仓不存在 / 无权访问」照旧先走各自的类，不许被"对象不在"吃掉。
fn is_transport_shaped(err: &git2::Error) -> bool {
    if matches!(
        err.class(),
        git2::ErrorClass::Http | git2::ErrorClass::Ssl | git2::ErrorClass::Ssh
    ) {
        return true;
    }
    let text = err.message().to_ascii_lowercase();
    if is_auth_shaped(err.code(), &text) || is_not_found_shaped(err.code(), &text) {
        return true;
    }
    TRANSPORT_PHRASES.iter().any(|phrase| text.contains(phrase))
}

/// 传输层失败的措辞（全部小写，按 `contains` 匹配）。
///
/// 收在这里而不是散在各处：这是一份**实测清单**，加新词要有据（见过一次真的网络失败长这样），
/// 不能凭想象扩。宁可少一条（那种失败会落到 [`commit_unavailable`]，报文仍指向"换一个 commit"
/// 而不是重试，方向不至于反）也不要多一条（那会把"那个 commit 取不到"重新混回网络类）。
const TRANSPORT_PHRASES: &[&str] = &[
    "socket",
    "broken pipe",
    "reset by peer",
    "unexpected eof",
    "failed to connect",
    "unable to connect",
    "could not resolve",
    "resolve host",
    "timed out",
    "timeout",
    "certificate",
    "tls",
    "ssl",
    "proxy",
    "requested url returned error",
];

/// 「读不到」的报文——**私有仓与不存在共用一个去处**。
///
/// **这一条是实测缺口，别把它当成已验的事**：无凭据去读一个私有仓时，GitHub 究竟回 401 系列
/// 还是干脆 404，尚未实测（票 02 把它列为「第一个要做的动作」）。若与「仓不存在」同形，
/// [`is_auth_shaped`] 就永远打不到，这一类会退化成 [`KIND_REPO_NOT_FOUND`]——而那一类的文案里
/// **已经写了「也可能是私有仓且无权访问」**，所以两条路都不会把用户引到错误的方向上。
/// 这个判定宁可漏报（落到 not_found），也不硬造一个判不出来的类。
fn unreadable(context: &str, repo: &RepoId, err: &git2::Error) -> Error {
    market_error(
        KIND_REPO_UNREADABLE,
        format!(
            "读不到仓 {}：GitHub 上没有这个仓，或它是一个**私有仓**——本版不支持私有仓，\
             也没有凭据入口。请确认 owner/repo 有没有拼错；若它确实是私有仓，请换一个公开仓",
            repo.slug()
        ),
        format!(
            "{context}：{} code={:?} msg={}",
            repo.slug(),
            err.code(),
            err.message()
        ),
    )
}

/// 权限形态的判定（401 / 403 / 认证失败）。见 [`unreadable`] 那条实测缺口的说明。
fn is_auth_shaped(code: git2::ErrorCode, text: &str) -> bool {
    matches!(code, git2::ErrorCode::Auth)
        || text.contains("401")
        || text.contains("403")
        || text.contains("authentication")
        || text.contains("not authorized")
        || text.contains("credential")
}

/// 「仓不存在」形态的判定（404 / repository not found）。
fn is_not_found_shaped(code: git2::ErrorCode, text: &str) -> bool {
    matches!(code, git2::ErrorCode::NotFound)
        || text.contains("404")
        || text.contains("not found")
        || text.contains("repository not exists")
}

fn not_found(context: &str, repo: &RepoId, err: &git2::Error) -> Error {
    market_error(
        KIND_REPO_NOT_FOUND,
        format!(
            "仓 {} 不存在（也可能是私有仓且无权访问——本版不支持私有仓）。\
             请检查 owner/repo 有没有拼错",
            repo.slug()
        ),
        format!(
            "{context}：{} code={:?} msg={}",
            repo.slug(),
            err.code(),
            err.message()
        ),
    )
}

// ─────────────────────────── 树的遍历 ───────────────────────────

/// 递归找「含 `SKILL.md` 的目录」。
///
/// **判据只有一条：basename 精确等于 `SKILL.md`。** 用 `endsWith('skill.md')` 或大小写不敏感
/// 的收尾匹配会把 `mattpocock/skills` 的 `.changeset/add-implement-spec-skill.md` 误收进来
/// （实测 38 vs 37 的差就是它）。
///
/// 深度无关：7 个流行仓 290/290 个技能满足「名字 = 含 `SKILL.md` 的那个目录的 basename」，
/// 深度 2–5 段都有（仓内各自统一）。主流 CLI 的 `getSkillFolderPath()` 用的是同一判据。
///
/// 仓根**本身**有 `SKILL.md` 时，那个技能的名字取仓名（它的目录 basename 就是仓名）。
fn walk_for_skills(
    handle: &Repository,
    tree: &git2::Tree<'_>,
    path: &str,
    repo_name: &str,
    depth: usize,
    out: &mut Vec<SkillRef>,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Ok(());
    }
    let mut skill_md: Option<git2::Oid> = None;
    let mut subtrees: Vec<(String, git2::Oid)> = Vec::new();
    for entry in tree.iter() {
        let Some(name) = entry.name() else { continue };
        match entry.kind() {
            Some(ObjectType::Blob) => {
                if name == SKILL_FILE {
                    skill_md = Some(entry.id());
                }
            }
            Some(ObjectType::Tree) => {
                let child = if path.is_empty() {
                    name.to_string()
                } else {
                    format!("{path}/{name}")
                };
                subtrees.push((child, entry.id()));
            }
            // 子模块（commit）与标签不参与：技能包是 markdown，不是子模块
            _ => {}
        }
    }
    if let Some(oid) = skill_md {
        let name = if path.is_empty() {
            repo_name.to_string()
        } else {
            path.rsplit('/').next().unwrap_or(repo_name).to_string()
        };
        out.push(SkillRef {
            dir: path.to_string(),
            name,
            description: read_description(handle, oid),
        });
    }
    for (child, oid) in subtrees {
        if let Ok(sub) = handle.find_tree(oid) {
            walk_for_skills(handle, &sub, &child, repo_name, depth + 1, out)?;
        }
    }
    Ok(())
}

/// 读一个 `SKILL.md` 的 frontmatter `description`（读不出来就 `None`——列表不该因为
/// 一个技能的描述坏了就整列出不来）。
fn read_description(handle: &Repository, oid: git2::Oid) -> Option<String> {
    let blob = handle.find_blob(oid).ok()?;
    if blob.size() > MAX_BLOB_BYTES {
        return None;
    }
    let raw = std::str::from_utf8(blob.content()).ok()?;
    parse_frontmatter(raw).0.description
}

/// 收集一棵子树下的全部普通文件（相对 `prefix` 的路径 → 内容），**递归**。
///
/// 递归是必须的：实测 pulumi 的技能目录里 `agents/` 是子树（`agents/openai.yaml`），
/// 非递归会漏掉兄弟文件，而兄弟文件的展开依赖它们真的落盘。
fn collect_files(
    handle: &Repository,
    tree: &git2::Tree<'_>,
    prefix: &str,
    depth: usize,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Ok(());
    }
    for entry in tree.iter() {
        let Some(name) = entry.name() else { continue };
        let rel = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Blob) => {
                let blob = handle.find_blob(entry.id()).map_err(|e| {
                    market_error(
                        KIND_COMMIT_NOT_FOUND,
                        format!("读不到技能里的文件 {rel}：{e}"),
                        format!("blob {rel}: {e}"),
                    )
                })?;
                if blob.size() > MAX_BLOB_BYTES {
                    return Err(market_error(
                        KIND_TOO_LARGE,
                        format!(
                            "技能里的文件过大（{rel}，{} 字节 > {MAX_BLOB_BYTES}）：\
                             技能是 markdown 目录，不该有这么大的单个文件",
                            blob.size()
                        ),
                        format!("blob = {rel}, size = {}", blob.size()),
                    ));
                }
                out.insert(rel, blob.content().to_vec());
            }
            Some(ObjectType::Tree) => {
                if let Ok(sub) = handle.find_tree(entry.id()) {
                    collect_files(handle, &sub, &rel, depth + 1, out)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// 把「名字 + 条目表」打成 `{name}/…` 的 store-only zip。
///
/// store（不压缩）：技能是几十 KB 的 markdown，压缩省不下什么，而 store 让这一步没有
/// 压缩器可以出错。
fn repack(name: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (rel, bytes) in files {
            writer
                .start_file(format!("{name}/{rel}"), opts)
                .map_err(|e| zip_error("写 zip 条目失败", rel, e))?;
            writer
                .write_all(bytes)
                .map_err(|e| zip_io_error("写 zip 内容失败", rel, e))?;
        }
        writer
            .finish()
            .map_err(|e| zip_error("收尾 zip 失败", name, e))?;
    }
    Ok(buf.into_inner())
}

fn open_error(path: &Path, err: git2::Error) -> Error {
    market_error(
        KIND_NETWORK,
        format!("打开已取下的仓失败（{}）：{err}", path.display()),
        format!("open {}: {err}", path.display()),
    )
}

fn zip_io_error(context: &str, key: &str, err: std::io::Error) -> Error {
    market_error(
        KIND_NETWORK,
        format!("在内存里重打技能包失败（{key}）：{err}"),
        format!("{context}: {key}: {err}"),
    )
}

fn zip_error(context: &str, key: &str, err: zip::result::ZipError) -> Error {
    market_error(
        KIND_NETWORK,
        format!("在内存里重打技能包失败（{key}）：{err}"),
        format!("{context}: {key}: {err}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    fn kind_of(err: &Error) -> Option<&str> {
        err.market_kind().map(|(kind, _)| kind)
    }

    // ── 哈希不符（离线 fixture 造不出来，故按措辞与 class 钉住）──

    #[test]
    fn digest_shaped_errors_are_recognised_by_class_or_phrase() {
        use git2::{Error as GError, ErrorClass, ErrorCode};
        // class 分得开：libgit2 用 SHA1 这一类报"对象哈希不对"
        assert!(is_digest_shaped(&GError::new(
            ErrorCode::GenericError,
            ErrorClass::Sha1,
            "whatever"
        )));
        // 措辞那一路（不同版本用不同 class 报同一件事）
        for msg in [
            "object hash mismatch",
            "loose object is corrupt",
            "invalid object header",
            "checksum mismatch",
        ] {
            assert!(
                is_digest_shaped(&GError::new(ErrorCode::GenericError, ErrorClass::Odb, msg)),
                "{msg} 应当被认作哈希不符"
            );
        }
        // 反面：协议层被拒与真连不上都不许掉进这一类
        for (class, msg) in [
            (ErrorClass::Net, "unexpected packet type"),
            (
                ErrorClass::Net,
                "error receiving data from socket: Broken pipe",
            ),
            (ErrorClass::Http, "unexpected eof"),
        ] {
            assert!(
                !is_digest_shaped(&GError::new(ErrorCode::GenericError, class, msg)),
                "{msg} 不该被认作哈希不符"
            );
        }
    }

    #[test]
    fn a_digest_failure_maps_to_the_digest_class() {
        let repo = RepoId::parse("acme/repo").unwrap();
        let commit = Oid::parse(A).unwrap();
        let raw = git2::Error::new(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Sha1,
            "hash mismatch",
        );
        let err = digest_mismatch(&repo, &commit, &raw);
        assert_eq!(kind_of(&err), Some(KIND_DIGEST));
        let msg = err.to_string();
        assert!(msg.contains("别装"), "报文要劝住「先别装」：{msg}");
        assert!(msg.contains("acme/repo"), "{msg}");
    }

    // ── RepoId 解析 ──

    #[test]
    fn repo_id_accepts_owner_slash_repo() {
        let id = RepoId::parse("obra/superpowers").unwrap();
        assert_eq!(id.owner, "obra");
        assert_eq!(id.name, "superpowers");
        assert_eq!(id.slug(), "obra/superpowers");
        assert_eq!(
            id.url(DEFAULT_GIT_BASE),
            "https://github.com/obra/superpowers.git"
        );
    }

    /// 粘贴残留：地址栏来的 `https://github.com/owner/repo` 与 clone 命令来的 `.git` 后缀。
    #[test]
    fn repo_id_strips_paste_residue() {
        for raw in [
            "https://github.com/obra/superpowers",
            "http://github.com/obra/superpowers",
            "github.com/obra/superpowers",
            "obra/superpowers.git",
            "obra/superpowers/",
            "  obra/superpowers  ",
            "GitHub.com/obra/superpowers.git",
        ] {
            let id = RepoId::parse(raw).unwrap_or_else(|e| panic!("{raw} 应被接受：{e}"));
            assert_eq!(id.slug(), "obra/superpowers", "raw = {raw}");
        }
    }

    /// 大小写**照收不改**：GitHub 认大小写不敏感，但 `Obra/Superpowers` 有展示意义。
    #[test]
    fn repo_id_preserves_case() {
        assert_eq!(
            RepoId::parse("Obra/Superpowers").unwrap().slug(),
            "Obra/Superpowers"
        );
    }

    #[test]
    fn repo_id_rejects_everything_that_could_steer_the_transport() {
        // 带 scheme、含 `@`、`..`、空段、多余斜杠、非 ASCII、空 owner/repo —— 一条不落。
        // 这些不是洁癖：URL 由我们拼，输入若能带 scheme 或路径段，走哪条 transport、
        // 读到哪个目录就由输入决定了。
        for raw in [
            "https://gitlab.com/obra/superpowers",
            "git://github.com/obra/superpowers",
            "ssh://git@github.com/obra/superpowers.git",
            "git@github.com:obra/superpowers.git",
            "file:///etc/passwd",
            "/etc/passwd",
            "../etc/passwd",
            "obra/../../etc",
            "obra//superpowers",
            "/obra/superpowers",
            "obra/superpowers/extra",
            "obra",
            "/",
            "",
            "   ",
            ".obra/superpowers",
            "-obra/superpowers",
            "obra/.hidden",
            "中文/技能",
            "obra/super powers",
        ] {
            assert!(
                RepoId::parse(raw).is_err(),
                "{raw:?} 应当被拒绝（它可能改变我们读哪里）"
            );
        }
    }

    #[test]
    fn bad_repo_names_report_the_repo_class() {
        let err = RepoId::parse("https://gitlab.com/a/b").unwrap_err();
        assert_eq!(kind_of(&err), Some(KIND_REPO_NOT_ALLOWED));
    }

    // ── Oid ──

    #[test]
    fn oid_requires_full_forty_hex() {
        let full = "553c2077f0edc3d5dc5d17262f6aa498e69d6f8e";
        let id = Oid::parse(full).unwrap();
        assert_eq!(id.as_str(), full);
        assert_eq!(id.short(), "553c207");
        // 大小写混写照收，归一成小写
        assert_eq!(Oid::parse(&full.to_uppercase()).unwrap().as_str(), full);
    }

    /// 缩写 SHA 是**静默空转**的原料：实测 7 位 SHA 会让 `fetch` 返回 `Ok` 却什么都不取。
    /// 不校验就会报「装好了」而其实没装。
    #[test]
    fn oid_rejects_abbreviated_sha() {
        for raw in ["553c207", "553c2077f0edc3d5dc5d17262f6aa498e69d6f8", ""] {
            let err = Oid::parse(raw).unwrap_err();
            assert_eq!(kind_of(&err), Some(KIND_COMMIT_NOT_FOUND));
            assert!(err.to_string().contains("40"), "{err}");
        }
        assert!(Oid::parse("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
    }

    // ── 白名单（唯一的安全控制） ──

    #[test]
    fn empty_allowlist_rejects_every_repo() {
        let id = RepoId::parse("obra/superpowers").unwrap();
        let err = repo_allowed(&id, &[]).unwrap_err();
        assert_eq!(kind_of(&err), Some(KIND_REPO_NOT_ALLOWED));
        // 报文要说清「怎么开」，而不是含糊的「内部错误」
        assert!(err.to_string().contains("技能市场"), "{err}");
    }

    #[test]
    fn allowlist_matches_slug_case_insensitively_but_not_by_prefix() {
        let id = RepoId::parse("obra/superpowers").unwrap();
        assert!(repo_allowed(&id, &["obra/superpowers".into()]).is_ok());
        assert!(repo_allowed(&id, &["Obra/Superpowers".into()]).is_ok());
        assert!(repo_allowed(&id, &["https://github.com/obra/superpowers".into()]).is_ok());
        // 前缀伪装不成立
        assert!(repo_allowed(&id, &["obra/superpowers-evil".into()]).is_err());
        assert!(repo_allowed(&id, &["obra".into()]).is_err());
        assert!(repo_allowed(&id, &["other/superpowers".into()]).is_err());
    }

    // ── 基础地址（测试接缝） ──

    #[test]
    fn git_base_accepts_https_and_loopback_http_only() {
        assert_eq!(
            normalize_git_base("https://github.com").unwrap(),
            "https://github.com"
        );
        assert_eq!(
            normalize_git_base("https://github.com/").unwrap(),
            "https://github.com"
        );
        assert_eq!(
            normalize_git_base("http://127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            normalize_git_base("http://localhost:8787").unwrap(),
            "http://localhost:8787"
        );
        assert_eq!(
            normalize_git_base("http://127.0.1.5:1").unwrap(),
            "http://127.0.1.5:1"
        );
        assert_eq!(
            normalize_git_base("http://[::1]:8787").unwrap(),
            "http://[::1]:8787"
        );
    }

    #[test]
    fn git_base_rejects_plain_http_off_loopback_and_paths() {
        for raw in [
            "http://github.com", // 非回环明文 http
            "http://skills.example.com",
            "https://github.com/x",     // 带路径
            "https://github.com/?q=1",  // 带查询
            "https://github.com/#frag", // 带片段
            "github.com",               // 没有 scheme
            "ftp://github.com",
            "https://user:pass@github.com",
            "",
        ] {
            assert!(normalize_git_base(raw).is_err(), "{raw:?} 应当被拒绝");
        }
    }

    // 回环谓词的表测试已随谓词迁到 `crate::host_policy`（共享 fixture 逐行断言，
    // 决策 246）——原先 `loopback_hosts_cover_the_whole_127_slash_8` 的七条断言
    // 全部由 `tests/fixtures/host_policy_loopback.json` 承接。

    // ── 默认分支的挑选（`head()` 唯一有分支的地方） ──

    fn heads(pairs: &[(&str, &str)]) -> Vec<(String, Oid)> {
        pairs
            .iter()
            .map(|(name, sha)| ((*name).to_string(), Oid::parse(sha).unwrap()))
            .collect()
    }

    #[test]
    fn default_tip_prefers_the_advertised_default_branch() {
        let list = heads(&[("refs/heads/main", A), ("refs/heads/release", B)]);
        assert_eq!(
            pick_default_tip(&list, Some("refs/heads/release"))
                .unwrap()
                .as_str(),
            B
        );
    }

    #[test]
    fn default_tip_falls_back_to_main_then_master_then_first_branch() {
        let list = heads(&[("refs/heads/master", B), ("refs/heads/main", A)]);
        assert_eq!(pick_default_tip(&list, None).unwrap().as_str(), A);
        let list = heads(&[("refs/heads/develop", B), ("refs/heads/master", A)]);
        assert_eq!(pick_default_tip(&list, None).unwrap().as_str(), A);
        let list = heads(&[("refs/heads/develop", B)]);
        assert_eq!(pick_default_tip(&list, None).unwrap().as_str(), B);
        // 广告里只有 tag / HEAD，没有分支 → 没有 tip 可给
        assert!(pick_default_tip(&heads(&[("HEAD", A)]), None).is_none());
        // symref 指向一个不在列表里的名字 → 回落，而不是返回 None
        assert_eq!(
            pick_default_tip(&heads(&[("refs/heads/main", A)]), Some("refs/heads/gone"))
                .unwrap()
                .as_str(),
            A
        );
    }

    // ── 技能目录路径 ──

    #[test]
    fn sanitize_dir_accepts_nested_paths_and_the_root() {
        assert_eq!(sanitize_dir("skills/grill").unwrap(), "skills/grill");
        assert_eq!(
            sanitize_dir("plugins/x/skills/y").unwrap(),
            "plugins/x/skills/y"
        );
        assert_eq!(sanitize_dir("./skills//grill/").unwrap(), "skills/grill");
        // 空串 = 仓根：仓根有 SKILL.md 时，那个仓本身就是一个技能
        assert_eq!(sanitize_dir("").unwrap(), "");
        assert_eq!(sanitize_dir("/").unwrap(), "");
    }

    #[test]
    fn sanitize_dir_rejects_traversal() {
        for raw in ["../etc", "skills/../../etc", "a\\..\\b", "C:/x"] {
            assert!(sanitize_dir(raw).is_err(), "{raw:?} 应当被拒绝");
        }
    }

    // ── 重打 zip ──

    /// 重打出来的包必须能过**既有那几道门**——这是「引擎零改动」这句话的可执行形式。
    #[test]
    fn repacked_bundle_passes_the_existing_gates() {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        files.insert(
            SKILL_FILE.to_string(),
            "---\ndescription: 拷问设计树\n---\n\n正文\n"
                .as_bytes()
                .to_vec(),
        );
        files.insert("agents/openai.yaml".to_string(), b"model: gpt\n".to_vec());
        // 归档噪声：`from_zip` 会把它过滤掉，不该出现在最终的素材里
        files.insert("__MACOSX/._SKILL.md".to_string(), b"junk".to_vec());

        let zip = repack("grill", &files).unwrap();
        let pkg = SkillPackage::from_zip(&zip, Some("grill")).unwrap();
        let info = pkg.validate().unwrap();
        assert_eq!(info.name, "grill");
        assert_eq!(info.description.as_deref(), Some("拷问设计树"));
        // 子树里的兄弟文件必须真的在包里（票 07 的展开依赖它）。条目名带 `{name}/` 前缀
        // 是**刻意的**：单根剥离发生在落盘时（`strip_prefix`），构造期保留全路径。
        assert!(
            pkg.files.contains_key("grill/agents/openai.yaml"),
            "{:?}",
            pkg.files.keys()
        );
        // 归档噪声被 `from_zip` 的 `is_archive_junk` 按段过滤（它查的是任意一段 == `__MACOSX`）
        assert!(
            !pkg.files.keys().any(|k| k.contains("__MACOSX")),
            "{:?}",
            pkg.files.keys()
        );
    }

    /// 「引擎零改动」不等于「引擎的门不在路上」：重打的字节仍要过 `from_zip` 的每一道。
    /// 这条喂一个坏名字给重打出来的**好包**，断言引擎那道仍然拒——即我们没有替引擎放行。
    #[test]
    fn repacked_bundle_still_goes_through_the_name_gate() {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        files.insert(
            SKILL_FILE.to_string(),
            "---\n---\n\n正文\n".as_bytes().to_vec(),
        );
        let zip = repack("evil", &files).unwrap();
        assert!(SkillPackage::from_zip(&zip, Some("../evil")).is_err());
        assert!(SkillPackage::from_zip(&zip, Some("evil")).is_ok());
    }

    // ── 失败分类 ──

    #[test]
    fn failure_kinds_are_eight_distinct_strings() {
        let all = [
            KIND_NETWORK,
            KIND_REPO_NOT_FOUND,
            KIND_COMMIT_NOT_FOUND,
            KIND_SKILL_NOT_FOUND,
            KIND_REPO_UNREADABLE,
            KIND_DIGEST,
            KIND_REPO_NOT_ALLOWED,
            KIND_TOO_LARGE,
        ];
        let mut sorted: Vec<&str> = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8, "八类失败不许撞名：{all:?}");
    }

    #[test]
    fn skill_not_found_names_the_directory_and_the_commit() {
        let repo = RepoId::parse("obra/superpowers").unwrap();
        let commit = Oid::parse(A).unwrap();
        let err = skill_not_found(&repo, &commit, "skills/nope");
        assert_eq!(kind_of(&err), Some(KIND_SKILL_NOT_FOUND));
        let msg = err.to_string();
        assert!(msg.contains("obra/superpowers"), "{msg}");
        assert!(msg.contains("1111111"), "{msg}");
        assert!(msg.contains("SKILL.md"), "{msg}");
    }

    /// 超限的报文必须**带我们自己记的数字**：中止错误串本身可能是空的
    /// （实测 GIT_EUSER 那条报 `class=None code=User msg=no error`）。
    #[test]
    fn too_large_reports_what_we_counted_ourselves() {
        let repo = RepoId::parse("obra/superpowers").unwrap();
        let commit = Oid::parse(A).unwrap();
        let msg = too_large(
            &repo,
            &commit,
            66560,
            200,
            "indexer progress callback returned -1",
        )
        .to_string();
        assert!(msg.contains("66560"), "须给出已收到的字节数：{msg}");
        assert!(msg.contains("200"), "须给出上限：{msg}");
        assert!(msg.contains("子目录"), "须给出可操作的去向：{msg}");
    }
}
