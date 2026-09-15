//! 技能市场：远程 registry 搜索与安装（决策 172⑤，票 10）。
//!
//! 本模块只做「**从哪拿**」与「**拿到的东西可不可信**」两件事，落盘交给票 09 的
//! [`crate::agent::skill_import`]——下完的 zip 走同一个 [`SkillPackage::from_zip`]，
//! 因此结构校验、同名冲突、路径穿越防护**一处生效、两处受益**。
//!
//! ## 索引格式（本票定义）
//!
//! `GET {source}/index.json`：
//!
//! ```json
//! {
//!   "skills": [
//!     {
//!       "name": "grill-me",
//!       "version": "1.2.0",
//!       "sha256": "9f86d0818…",
//!       "source": "https://skills.example.com",
//!       "description": "拷问设计树",
//!       "url": "https://skills.example.com/skills/grill-me-1.2.0.zip"
//!     }
//!   ]
//! }
//! ```
//!
//! 五个字段对应票面的「索引条目含技能名、版本、`sha256`、来源、描述」；`url` 是下载地址，
//! 单独给是因为它允许包与索引**不同源**（常见于 CDN）——而正因如此，安装时必须校验
//! `url` 的 origin 也在白名单里（见下）。
//!
//! ## 唯一新增接缝：`MarketClient`（决策 143）
//!
//! 本 effort 只新增这一条接缝。测试用 fake 提供固定字节与摘要，**不打真网络**——
//! `cargo test` 在飞机上也能跑，且「摘要不符」「来源未放行」「索引畸形」「网络失败」
//! 这四条错误路径本来就没法用真网络稳定复现。
//!
//! ## 五类失败互不混淆（票面显式要求「网络失败不与摘要不符 / 来源未放行混为一谈」）
//!
//! `market_network` / `market_digest_mismatch` / `market_source_not_allowed` /
//! `market_index_malformed` / `market_not_found`。混在一起的代价是用户不知道该改什么：
//! 网络失败该重试、摘要不符该怀疑中间人、来源未放行该改配置、索引畸形该找 registry
//! 维护者、技能不存在该换个名字——五种动作毫无交集。故 [`Error::Market`] 带稳定 `kind`，
//! 与 [`Error::LlmClassified`] 同一姿态（原始诊断进 `raw`，中文可操作提示进 `message`）。
//! API 层按 `kind` 分派状态码（502 / 404 / 400），见 `crates/app/src/routes/market.rs`。
//!
//! ## 明确不做
//!
//! **签名与人工审核队列**。本批只有摘要 + 白名单；装前预览与信任标记是票 11。摘要校验
//! **只能**证明「没被改过」，证明不了「内容是善意的」——后者由票 11 的预览与信任标记承担。
//! 这条边界写在本文档与 `docs/agents.md` 里，否则用户会误以为摘要=安全。

use std::path::Path;

use futures::future::BoxFuture;

use crate::agent::skill_import::{self, PackageInfo};
use crate::config::normalize_origin;
use crate::error::{Error, Result};

/// 失败类别：连不上（DNS / 连接被拒 / TLS / 超时 / 非 2xx）。
pub const KIND_NETWORK: &str = "market_network";
/// 失败类别：下载内容的 `sha256` 与索引声明的不符（可能被篡改或索引过期）。
pub const KIND_DIGEST: &str = "market_digest_mismatch";
/// 失败类别：来源未在 `[market] allowed_sources` 内放行。
pub const KIND_SOURCE: &str = "market_source_not_allowed";
/// 失败类别：索引本身不合法（非 JSON / 缺字段 / 摘要不是 64 位十六进制）。
pub const KIND_INDEX: &str = "market_index_malformed";
/// 失败类别：索引里没有要找的技能。
pub const KIND_NOT_FOUND: &str = "market_not_found";

/// 单个技能包的下载体积上限（64 MiB）。
///
/// 与票 09 上传端点的 `DefaultBodyLimit` 同值，使**远程与本地两条路对内存的消耗同量级**
/// （票面的意图是「远程包不比本地上传的包享有更宽的路」；本地那道限制在 HTTP 体上，
/// 远程若没有对等的一道，就是一处不对称）。对「一个 markdown 技能目录的 zip」有
/// 三个数量级余量。此上限只管**下载**；解压侧的上限另由票 09 的 `MAX_ENTRY_BYTES` /
/// `MAX_ENTRIES` 兜住。
const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

/// 索引里的一条技能条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: String,
    /// 版本（字符串，本系统只用于展示与去重，不参与比较）。
    pub version: String,
    /// 期望的包摘要（小写十六进制 sha256）。
    pub sha256: String,
    /// 来源 origin（`scheme://host[:port]`）。
    pub source: String,
    pub description: Option<String>,
    /// 包下载地址。**可跨源**（CDN），故安装时另校验它的 origin。
    pub url: String,
}

/// 一次下载的结果：字节 + 客户端声明的摘要 + 实际来源 origin。
///
/// `sha256` 是**传输层声明**的值，不是权威值：权威摘要由本模块从 `bytes` 现算
/// （见 [`verify_digest`]）。两者分歧本身就是值得报出来的信号，而不是取其一。
///
/// `source` 是**字节真正来自哪里**的 origin。它与请求的 URL 未必一致——重定向会让内容
/// 落在别处，故实现必须如实上报**最终**地址的 origin（见 [`install_from_market`] 的第三步）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub bytes: Vec<u8>,
    /// 客户端（registry / 传输层）声明的摘要。
    pub sha256: String,
    /// 实际来源 origin（字节真正来自哪里，供白名单复检）。
    pub source: String,
}

/// 市场客户端——**本 effort 唯一新增的可测试性接缝**（决策 143，票 10）。
///
/// 生产实现走 reqwest（复用既有 HTTP 栈，不引入第二套）；测试用 fake 提供固定字节与摘要。
/// 两个方法都返回 `BoxFuture<'static>` 以便对象安全（与 [`crate::agent::tools::SubAgentRunner`]
/// 同一姿态）。
pub trait MarketClient: Send + Sync + 'static {
    /// 拉取并解析索引。调用方负责把结果与关键词、白名单做筛选。
    fn index(&self) -> BoxFuture<'static, Result<Vec<IndexEntry>>>;
    /// 下载一个包。
    fn download(&self, url: &str) -> BoxFuture<'static, Result<Downloaded>>;
}

/// 按关键词筛选候选（名字或描述命中；空关键词 = 全部）。
///
/// `allowed_sources` 非空时**未放行的来源不进候选**——让用户看不到装不上的东西，
/// 比让他点了再报错好。放行集合为空 = 未配置任何来源，此时返回空清单（默认拒绝远程安装）。
///
/// 判定同时覆盖条目的 `source` 与**下载地址的 origin**：只查前者会让「`source` 放行、
/// `url` 指向别处」的条目出现在候选里，用户点了才发现装不上——那正是候选侧要避免的情形，
/// 而安装侧本来就会拒它（两道判定必须同口径）。
pub fn search(
    entries: Vec<IndexEntry>,
    query: &str,
    allowed_sources: &[String],
) -> Vec<IndexEntry> {
    let needle = query.trim().to_lowercase();
    entries
        .into_iter()
        .filter(|e| source_allowed(&e.source, allowed_sources).is_ok())
        .filter(|e| match origin_of(&e.url) {
            Some(origin) => source_allowed(&origin, allowed_sources).is_ok(),
            // 下载地址不是合法 http(s) URL：一条装不上的条目，同样不该进候选
            None => false,
        })
        .filter(|e| {
            if needle.is_empty() {
                return true;
            }
            e.name.to_lowercase().contains(&needle)
                || e.description
                    .as_deref()
                    .is_some_and(|d| d.to_lowercase().contains(&needle))
        })
        .collect()
}

/// 来源是否放行。判定用 origin（`scheme://host[:port]`），与决策 157 的
/// `[server] allowed_origins` 同一套解析——**不接受带路径的写法**，避免
/// 「白名单写了个前缀，结果放行了同主机的全部路径」这种错觉。
///
/// 空白名单 = **不放行任何来源**（票面「默认只放行配置内的源」）。这是保守方向上的默认：
/// 忘配的代价是装不上（用户立刻发现），配宽的代价是静默装上陌生来源。
pub fn source_allowed(source: &str, allowed_sources: &[String]) -> Result<()> {
    let actual = normalize_origin(source)
        .map_err(|e| Error::Validation(format!("来源不是合法 origin：{source}（{e}）")))?;
    if allowed_sources.is_empty() {
        return Err(Error::Market {
            kind: KIND_SOURCE.into(),
            message: format!(
                "未放行任何技能来源：请在配置的 [market] allowed_sources 里加入 {actual} \
                 （默认空 = 不允许远程安装）"
            ),
            raw: format!("source = {actual}"),
        });
    }
    let matched = allowed_sources.iter().any(|allowed| {
        normalize_origin(allowed)
            .map(|a| a == actual)
            .unwrap_or(false)
    });
    if !matched {
        return Err(Error::Market {
            kind: KIND_SOURCE.into(),
            message: format!(
                "技能来源未放行：{actual}（请在 [market] allowed_sources 里显式加入该来源）"
            ),
            raw: format!("source = {actual}, allowed = {allowed_sources:?}"),
        });
    }
    Ok(())
}

/// 校验下载内容的摘要（票 10 的核心）。
///
/// **权威摘要是现算的**，不是取自传输层：`downloaded.sha256` 由客户端上报，若采信它，
/// 一个被控制的客户端可以同时改内容与声称值，校验就形同虚设。故：
/// - `actual` = 对 `bytes` 现算的 sha256；
/// - `expected` = 索引里钉住的 sha256；
/// - 两者不符 → [`KIND_DIGEST`]，报文**同时给出期望值与实际值**（票面显式要求）。
///
/// 客户端上报值与现算值不一致时也报错（额外的篡改信号，零成本）。
pub fn verify_digest(downloaded: &Downloaded, expected: &str) -> Result<()> {
    let actual = sha256_hex(&downloaded.bytes);
    let expected = expected.trim().to_lowercase();
    if actual != expected {
        return Err(Error::Market {
            kind: KIND_DIGEST.into(),
            message: format!(
                "技能包摘要不符，已拒绝安装：期望 {expected}，实际 {actual}。\
                 内容可能被篡改，或索引里的摘要已过期——请与来源方核对后再装"
            ),
            raw: format!("expected = {expected}, actual = {actual}"),
        });
    }
    let claimed = downloaded.sha256.trim().to_lowercase();
    if !claimed.is_empty() && claimed != actual {
        return Err(Error::Market {
            kind: KIND_DIGEST.into(),
            message: format!(
                "技能包的传输层摘要自相矛盾，已拒绝安装：传输声明 {claimed}，现算 {actual}"
            ),
            raw: format!("claimed = {claimed}, computed = {actual}"),
        });
    }
    Ok(())
}

/// sha256 → 小写十六进制。
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 解析索引 JSON。
///
/// **每一条的缺失字段都是 [`KIND_INDEX`]**（fail fast）而不是跳过：一条缺 `sha256` 的条目
/// 若被静默跳过，用户会看到「搜索不到我要的技能」而不是「这个 registry 的索引坏了」——
/// 那是把 registry 的问题伪装成「没有这个技能」，排查方向完全错。
pub fn parse_index(bytes: &[u8]) -> Result<Vec<IndexEntry>> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| Error::Market {
        kind: KIND_INDEX.into(),
        message: format!("技能索引不是合法 JSON：{e}"),
        raw: String::from_utf8_lossy(&bytes[..bytes.len().min(500)]).to_string(),
    })?;
    let items = value
        .get("skills")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Market {
            kind: KIND_INDEX.into(),
            message: "技能索引缺少 skills 数组（格式见 docs/agents.md 的技能市场一节）".into(),
            raw: value.to_string(),
        })?;

    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let field = |key: &str| -> Result<String> {
            item.get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| Error::Market {
                    kind: KIND_INDEX.into(),
                    message: format!("技能索引第 {i} 条缺少 {key} 字段"),
                    raw: item.to_string(),
                })
        };
        let name = field("name")?;
        let version = field("version")?;
        let sha256 = field("sha256")?.trim().to_lowercase();
        let source = field("source")?;
        let url = field("url")?;
        // 摘要在下载**之前**就能判非法：早点报「索引坏了」，而不是让用户下完几十兆才失败
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Market {
                kind: KIND_INDEX.into(),
                message: format!(
                    "技能索引第 {i} 条（{name}）的 sha256 不是 64 位十六进制：{sha256}"
                ),
                raw: item.to_string(),
            });
        }
        if normalize_origin(&source).is_err() {
            return Err(Error::Market {
                kind: KIND_INDEX.into(),
                message: format!("技能索引第 {i} 条（{name}）的 source 不是合法 origin：{source}"),
                raw: item.to_string(),
            });
        }
        out.push(IndexEntry {
            name,
            version,
            sha256,
            source,
            description: item
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            url,
        });
    }
    Ok(out)
}

/// 从索引里挑出要装的那条：名字精确匹配，多条取**第一条**（索引顺序即优先级）。
pub fn pick_entry(entries: &[IndexEntry], name: &str) -> Result<IndexEntry> {
    entries
        .iter()
        .find(|e| e.name == name)
        .cloned()
        .ok_or_else(|| Error::Market {
            kind: KIND_NOT_FOUND.into(),
            message: format!("来源索引里没有技能：{name}"),
            raw: format!(
                "available = {:?}",
                entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>()
            ),
        })
}

/// 从市场安装一个技能（票 10 的主入口）。
///
/// 顺序刻意如此——**每一步都尽量在下载之前失败**：
/// 1. 索引里找到条目；
/// 2. 校验条目的 `source` 放行；
/// 3. 校验**下载地址的 origin** 也放行（包与索引可不同源，故这一步不能省）；
/// 4. 下载，并复检**字节实际来源**的 origin（挡重定向）；
/// 5. 校验摘要；
/// 6. 交给票 09 的 [`skill_import::install`] 落盘。
///
/// 第 6 步复用票 09 的全部防护：结构校验、同名冲突、路径穿越（zip 条目名与 realpath）。
pub async fn install_from_market(
    client: &dyn MarketClient,
    skills_root: &Path,
    name: &str,
    allowed_sources: &[String],
    overwrite: bool,
) -> Result<PackageInfo> {
    // 空白名单 = 不允许任何远程安装。这条判定放在**最前面**（早于拉索引），使「默认拒绝」
    // 成为本函数的**结构性质**，而不是只靠上层记得别构造客户端——少一次对外请求，
    // 也少一份「上层漏判就静默联网」的可能。
    if allowed_sources.is_empty() {
        return Err(Error::Market {
            kind: KIND_SOURCE.into(),
            message: "未放行任何技能来源，已拒绝远程安装：请在配置的 [market] allowed_sources 里\
                      加入可信来源的 origin（默认空 = 不允许远程安装）。本地导入不受影响"
                .to_string(),
            raw: format!("requested = {name}, allowed = []"),
        });
    }

    let entries = client.index().await?;
    let entry = pick_entry(&entries, name)?;
    source_allowed(&entry.source, allowed_sources)?;

    // 索引声明的名字必须在**下载之前**就是可落盘的：否则一个 `../x` 这类名字会先烧掉一次
    // 下载，再在落盘时才被票 09 的名字不变量拒掉——与「摘要格式早校验」同一理由
    // （早报错好过让用户下完几十兆才失败）。判定复用票 09 的同一条不变量，不另写一套。
    let checked_name =
        skill_import::check_skill_name(&entry.name).map_err(|reason| Error::Market {
            kind: KIND_INDEX.into(),
            message: format!("技能索引里的名字不能用作目录名（{reason}）：{}", entry.name),
            raw: entry.name.clone(),
        })?;

    // 下载地址的 origin 单独校验：索引可以把包指到别处（CDN），那正是白名单要拦的地方
    let url_origin = origin_of(&entry.url).ok_or_else(|| Error::Market {
        kind: KIND_INDEX.into(),
        message: format!("技能 {name} 的下载地址不是合法 URL：{}", entry.url),
        raw: entry.url.clone(),
    })?;
    source_allowed(&url_origin, allowed_sources)?;

    let downloaded = client.download(&entry.url).await?;
    // 复检**字节实际来源**：请求的 URL 放行不等于字节来自那里。HTTP 重定向可以让一个
    // 已放行的来源把内容指到任何别处（内网元数据端点之类），故这一步与上一步是**两道**
    // 判定。空 source 视为「客户端未上报」而跳过——与 `verify_digest` 对空声明摘要的宽容一致。
    if !downloaded.source.trim().is_empty() {
        source_allowed(&downloaded.source, allowed_sources)?;
    }
    verify_digest(&downloaded, &entry.sha256)?;

    let package = skill_import::SkillPackage::from_zip(&downloaded.bytes, Some(&checked_name))?;
    // 索引声明的名字与包内 frontmatter 的 name 由 `install` 一并校验（票 09 的同一条不变量）
    skill_import::install(skills_root, &package, overwrite)
}

/// 取一个 URL 的 origin（`scheme://host[:port]`）。
///
/// 只接受 http/https，且**要求有 host**：`file:///etc/passwd` 这类本地协议在远程安装的
/// 语境下毫无正当用途，放行它等于把「下载」变成一次任意文件读。
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        // 带 userinfo 的 URL（`https://user:pass@host`）不认：origin 判定会被它绕过
        return None;
    }
    normalize_origin(&format!("{scheme}://{authority}")).ok()
}

/// 生产实现：reqwest（复用既有 HTTP 栈，不引入第二套客户端，票 10 Notes）。
///
/// 索引地址取自 `[market] allowed_sources` 的**第一个**来源（`{source}/index.json`）；
/// 下载走条目自己的 `url`。超时统一 15s——技能包是几十 KB 量级，超过这个数说明网络
/// 或来源有问题，早失败早报错好过界面挂着。
///
/// ## 不跟随重定向（安全，非性能）
///
/// reqwest 缺省 `Policy::limited(10)`，即最多跟 10 跳**跨源**重定向。白名单判定看的是请求的
/// URL，因此一个**已放行**的来源只要回一个 302 就能把内容指到任意别处（内网元数据端点之类），
/// 白名单当场失效。故这里显式 `Policy::none()`：**请求即最终地址**，判定与实际来源必然一致。
/// 来源方若需要 CDN，应把 CDN 的 origin 直接写进索引条目的 `url`——那时的 origin 会被
/// [`install_from_market`] 的第三步正常校验，而不是靠一次隐式跳转绕过它。
pub struct HttpMarketClient {
    http: reqwest::Client,
    index_url: String,
}

impl HttpMarketClient {
    /// 按来源 origin 构造（`{source}/index.json`）。
    pub fn new(source: &str) -> Result<Self> {
        let origin = normalize_origin(source)
            .map_err(|e| Error::Validation(format!("技能来源不合法：{source}（{e}）")))?;
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Validation(format!("HTTP 客户端构建失败：{e}")))?;
        Ok(HttpMarketClient {
            http,
            index_url: format!("{origin}/index.json"),
        })
    }
}

/// 把 reqwest 的失败归类为可归因的网络错误（超时 / DNS / 连接 / HTTP 状态）。
///
/// 四类市场失败互不混淆是票面要求，故这里只产出 [`KIND_NETWORK`]——摘要不符与来源未放行
/// 各有各的判定点，绝不会走到这里。
fn network_error(context: &str, url: &str, err: reqwest::Error) -> Error {
    let detail = if err.is_timeout() {
        "请求超时"
    } else if err.is_connect() {
        "连不上（DNS 解析失败 / 连接被拒 / TLS 握手失败）"
    } else if err.is_decode() {
        "响应无法解析"
    } else {
        "请求失败"
    };
    Error::Market {
        kind: KIND_NETWORK.into(),
        message: format!(
            "技能市场网络失败（{detail}）：{context}。请检查网络与来源地址是否可达；\
             本机无网时本地导入（票 09）不受影响"
        ),
        raw: format!("{url}：{err}"),
    }
}

impl MarketClient for HttpMarketClient {
    fn index(&self) -> BoxFuture<'static, Result<Vec<IndexEntry>>> {
        let http = self.http.clone();
        let url = self.index_url.clone();
        Box::pin(async move {
            let response = http
                .get(&url)
                .send()
                .await
                .map_err(|e| network_error("拉取技能索引", &url, e))?;
            let status = response.status();
            if !status.is_success() {
                return Err(Error::Market {
                    kind: KIND_NETWORK.into(),
                    message: format!("拉取技能索引失败：HTTP {status}（{url}）"),
                    raw: format!("HTTP {status}"),
                });
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|e| network_error("读取技能索引响应", &url, e))?;
            parse_index(&bytes)
        })
    }

    fn download(&self, url: &str) -> BoxFuture<'static, Result<Downloaded>> {
        let http = self.http.clone();
        let url = url.to_string();
        Box::pin(async move {
            let response = http
                .get(&url)
                .send()
                .await
                .map_err(|e| network_error("下载技能包", &url, e))?;
            let status = response.status();
            // 不跟随重定向（`Policy::none()`），因此 3xx 是**失败**而不是要被跟下去的跳转。
            // 报错文案点明这一点：来源方若真需要 CDN，应把该 origin 写进索引的 `url`。
            if !status.is_success() {
                let hint = if status.is_redirection() {
                    "（重定向不被跟随：请让来源直接给出最终地址，并把该来源 origin 加入白名单）"
                } else {
                    ""
                };
                return Err(Error::Market {
                    kind: KIND_NETWORK.into(),
                    message: format!("下载技能包失败：HTTP {status}（{url}）{hint}"),
                    raw: format!("HTTP {status}"),
                });
            }
            // 声明式上限 + 流式累加：只信 `Content-Length` 等于把内存交给对面，
            // 故两处都设——一处早退，一处兜底（与票 09 的 MAX_ENTRY_BYTES 同一姿态）。
            if let Some(len) = response.content_length() {
                if len > MAX_DOWNLOAD_BYTES as u64 {
                    return Err(Error::Market {
                        kind: KIND_NETWORK.into(),
                        message: format!(
                            "技能包过大（{len} 字节 > {MAX_DOWNLOAD_BYTES}）：{url}。\
                             技能包是几十 KB 量级的 markdown 目录，这个体积不正常"
                        ),
                        raw: format!("content-length = {len}"),
                    });
                }
            }
            let source = origin_of(&url).unwrap_or_else(|| url.clone());
            use futures::StreamExt;
            let mut stream = response.bytes_stream();
            let mut bytes: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| network_error("读取技能包响应", &url, e))?;
                if bytes.len() + chunk.len() > MAX_DOWNLOAD_BYTES {
                    // 对面没给 `Content-Length`（或给了假的）时的兜底
                    return Err(Error::Market {
                        kind: KIND_NETWORK.into(),
                        message: format!(
                            "技能包超过 {MAX_DOWNLOAD_BYTES} 字节上限，已中断下载：{url}"
                        ),
                        raw: format!("read > {MAX_DOWNLOAD_BYTES}"),
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            // 本实现**不从传输层取任何声明摘要**（权威值由 `verify_digest` 现算，
            // 见函数文档）；这里填现算值是诚实上报，而非采信外部输入。
            let credible = sha256_hex(&bytes);
            Ok(Downloaded {
                bytes,
                sha256: credible,
                source,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "https://skills.example.com";

    fn allowed() -> Vec<String> {
        vec![SOURCE.to_string()]
    }

    /// 造一个单技能 zip（`{name}/SKILL.md`），用于算摘要。
    fn write_zip(name: &str, body: &str) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::SimpleFileOptions = Default::default();
            w.start_file(format!("{name}/SKILL.md"), opts).unwrap();
            w.write_all(format!("---\nname: {name}\n---\n\n{body}\n").as_bytes())
                .unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    /// 一条摘要与内容一致的索引条目。
    ///
    /// 本模块的单测只覆盖**纯函数**（解析 / 筛选 / 白名单 / 摘要）；端到端那些要驱动
    /// `install_from_market` 的用例在 `crates/core/tests/market.rs`——它们用 testkit 的
    /// 同一个 fake，而 testkit 经 dev-dependency 链接了**另一份** core，类型过不来。
    fn entry(name: &str, bytes: &[u8]) -> IndexEntry {
        IndexEntry {
            name: name.into(),
            version: "1.0.0".into(),
            sha256: sha256_hex(bytes),
            source: SOURCE.into(),
            description: Some("拷问设计树".into()),
            url: format!("{SOURCE}/skills/{name}-1.0.0.zip"),
        }
    }

    fn index_json(entries: &[IndexEntry]) -> Vec<u8> {
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "version": e.version,
                    "sha256": e.sha256,
                    "source": e.source,
                    "description": e.description,
                    "url": e.url,
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({ "skills": items })).unwrap()
    }

    // ── 索引格式 ──

    #[test]
    fn index_round_trips_five_fields() {
        let bytes = write_zip("grill", "正文");
        let entries = parse_index(&index_json(&[entry("grill", &bytes)])).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.name, "grill");
        assert_eq!(e.version, "1.0.0");
        assert_eq!(e.sha256.len(), 64);
        assert_eq!(e.source, SOURCE);
        assert_eq!(e.description.as_deref(), Some("拷问设计树"));
        assert!(e.url.ends_with("grill-1.0.0.zip"));
    }

    #[test]
    fn index_without_skills_array_is_malformed() {
        let err = parse_index(br#"{"items":[]}"#).unwrap_err();
        assert!(market_kind(&err) == Some(KIND_INDEX), "{err:?}");
    }

    #[test]
    fn index_not_json_is_malformed() {
        let err = parse_index(b"<html>404</html>").unwrap_err();
        assert!(market_kind(&err) == Some(KIND_INDEX), "{err:?}");
    }

    /// 缺字段须 fail fast（而不是静默跳过该条）——否则 registry 坏了会表现成「没有这个技能」。
    #[test]
    fn index_entry_missing_field_is_malformed() {
        let bad = br#"{"skills":[{"name":"a","version":"1","source":"https://x.example"}]}"#;
        let err = parse_index(bad).unwrap_err();
        assert!(market_kind(&err) == Some(KIND_INDEX), "{err:?}");
        assert!(err.to_string().contains("sha256"), "{err}");
    }

    /// 摘要不是 64 位十六进制 → 在**下载之前**就报索引畸形。
    #[test]
    fn index_with_bogus_digest_is_malformed() {
        let bad = br#"{"skills":[{"name":"a","version":"1","sha256":"zz","source":"https://x.example","url":"https://x.example/a.zip"}]}"#;
        let err = parse_index(bad).unwrap_err();
        assert!(market_kind(&err) == Some(KIND_INDEX), "{err:?}");
        assert!(err.to_string().contains("64"), "{err}");
    }

    #[test]
    fn index_with_illegal_source_is_malformed() {
        let bad = br#"{"skills":[{"name":"a","version":"1","sha256":"0000000000000000000000000000000000000000000000000000000000000000","source":"not-an-origin","url":"https://x.example/a.zip"}]}"#;
        let err = parse_index(bad).unwrap_err();
        assert!(market_kind(&err) == Some(KIND_INDEX), "{err:?}");
    }

    // ── 搜索 ──

    #[test]
    fn search_filters_by_name_and_description() {
        let bytes = write_zip("grill", "正文");
        let mut other = entry("tdd", &bytes);
        other.description = Some("测试驱动".into());
        let entries = vec![entry("grill", &bytes), other];

        assert_eq!(search(entries.clone(), "", &allowed()).len(), 2);
        let hits = search(entries.clone(), "gri", &allowed());
        let by_name: Vec<&str> = hits.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(by_name, vec!["grill"]);
        let hits2 = search(entries.clone(), "测试", &allowed());
        let by_desc: Vec<&str> = hits2.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(by_desc, vec!["tdd"]);
        assert!(search(entries, "nope", &allowed()).is_empty());
    }

    /// 未放行的来源不进候选——让用户看不到装不上的东西。
    #[test]
    fn search_hides_entries_from_unallowed_sources() {
        let bytes = write_zip("grill", "正文");
        let mut rogue = entry("rogue", &bytes);
        rogue.source = "https://evil.example".into();
        let entries = vec![entry("grill", &bytes), rogue];
        let hits3 = search(entries, "", &allowed());
        let names: Vec<&str> = hits3.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["grill"], "未放行来源不得出现在候选里");
    }

    #[test]
    fn search_with_empty_allowlist_returns_nothing() {
        let bytes = write_zip("grill", "正文");
        // 默认空白名单 = 不允许远程安装
        assert!(search(vec![entry("grill", &bytes)], "", &[]).is_empty());
    }

    /// `source` 放行但**下载地址**指向别处的条目也不进候选。
    ///
    /// 搜索与安装必须同口径：只查 `source` 会让这类装不上的条目出现在候选里，
    /// 用户点了才发现被拒——而候选侧存在的意义正是「看不到装不上的东西」。
    #[test]
    fn search_hides_entries_whose_download_url_is_unlisted() {
        let bytes = write_zip("grill", "正文");
        let mut redirected = entry("redirected", &bytes);
        redirected.url = "https://cdn.evil.example/redirected.zip".into();
        // 另一个条目的 url 根本不是合法 http(s) 地址
        let mut bogus = entry("bogus", &bytes);
        bogus.url = "file:///etc/passwd".into();

        let entries = vec![entry("grill", &bytes), redirected, bogus];
        let hits = search(entries, "", &allowed());
        let names: Vec<&str> = hits.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["grill"], "装不上的条目不得进候选");
    }

    // ── 来源白名单 ──

    #[test]
    fn default_empty_allowlist_rejects_every_source() {
        let err = source_allowed(SOURCE, &[]).unwrap_err();
        assert_eq!(market_kind(&err), Some(KIND_SOURCE));
        assert!(err.to_string().contains("allowed_sources"), "{err}");
    }

    #[test]
    fn unlisted_source_is_rejected_with_actionable_message() {
        let err = source_allowed("https://evil.example", &allowed()).unwrap_err();
        assert_eq!(market_kind(&err), Some(KIND_SOURCE));
        assert!(err.to_string().contains("evil.example"), "{err}");
    }

    #[test]
    fn allowlist_match_is_origin_wise_not_prefix_wise() {
        // 同主机不同端口是不同来源；大小写与尾斜杠归一后仍匹配
        assert!(source_allowed(SOURCE, &allowed()).is_ok());
        assert!(source_allowed("https://SKILLS.example.com/", &allowed()).is_ok());
        assert!(source_allowed("https://skills.example.com:8443", &allowed()).is_err());
        // 前缀伪装不成立
        assert!(source_allowed("https://skills.example.com.evil.test", &allowed()).is_err());
    }

    // ── 摘要校验 ──

    #[test]
    fn matching_digest_passes() {
        let bytes = write_zip("grill", "正文");
        let downloaded = Downloaded {
            bytes: bytes.clone(),
            sha256: sha256_hex(&bytes),
            source: SOURCE.into(),
        };
        assert!(verify_digest(&downloaded, &sha256_hex(&bytes)).is_ok());
    }

    /// 摘要不符 → 拒绝，且报文**同时给出期望值与实际值**（票面显式要求）。
    #[test]
    fn digest_mismatch_reports_expected_and_actual() {
        let bytes = write_zip("grill", "正文");
        let expected = "0".repeat(64);
        let downloaded = Downloaded {
            bytes: bytes.clone(),
            sha256: sha256_hex(&bytes),
            source: SOURCE.into(),
        };
        let err = verify_digest(&downloaded, &expected).unwrap_err();
        assert_eq!(market_kind(&err), Some(KIND_DIGEST));
        let msg = err.to_string();
        assert!(msg.contains(&expected), "须给出期望值：{msg}");
        assert!(msg.contains(&sha256_hex(&bytes)), "须给出实际值：{msg}");
    }

    /// 传输层声明的摘要与现算不符 → 同样拒绝（额外的篡改信号）。
    #[test]
    fn lying_transport_digest_is_rejected() {
        let bytes = write_zip("grill", "正文");
        let downloaded = Downloaded {
            bytes: bytes.clone(),
            sha256: "f".repeat(64),
            source: SOURCE.into(),
        };
        let err = verify_digest(&downloaded, &sha256_hex(&bytes)).unwrap_err();
        assert_eq!(market_kind(&err), Some(KIND_DIGEST));
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        // "abc" 的 sha256（NIST 向量），确认摘要算法接对了
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // ── URL origin 解析 ──

    #[test]
    fn origin_of_accepts_http_and_https_only() {
        assert_eq!(
            origin_of("https://skills.example.com/a/b.zip?x=1").as_deref(),
            Some("https://skills.example.com")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8788/x.zip").as_deref(),
            Some("http://127.0.0.1:8788")
        );
        // 本地协议与带 userinfo 的 URL 一律不认
        assert_eq!(origin_of("file:///etc/passwd"), None);
        assert_eq!(
            origin_of("https://user:pass@skills.example.com/x.zip"),
            None
        );
        assert_eq!(origin_of("ftp://x.example/a.zip"), None);
        assert_eq!(origin_of("not a url"), None);
    }

    /// 从 Error 里取市场失败类别（复用 `Error` 上的访问器，不在测试里另写一遍 match）。
    fn market_kind(err: &Error) -> Option<&str> {
        err.market_kind().map(|(kind, _)| kind)
    }
}
