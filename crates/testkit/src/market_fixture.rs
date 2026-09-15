//! 技能市场的 fake 客户端（决策 143 的唯一新增接缝，票 10）。
//!
//! [`FakeMarket`] 提供**固定索引与固定字节**，因此「摘要不符 / 来源未放行 / 索引畸形 /
//! 网络失败」这四条错误路径在 `cargo test` 里可稳定复现——用真网络复现它们本来就不可能
//! （要么得搭一个会返回坏摘要的服务器，要么得盯着偶发的连接失败）。
//!
//! 放在 testkit 而非 core 的内联单测里，是因为**两层都要用它**：core 的 L2 集成测试
//! （`crates/core/tests/market.rs`）驱动 `install_from_market`，L3 契约测试把它注入
//! `AppState` 驱动端点。一个 fake、两层受益，与 [`crate::FakeAgent`] / [`crate::MockLlm`]
//! 同一体例。
//!
//! （core 的内联 `#[cfg(test)] mod tests` 用不了本模块：testkit 经 dev-dependency 链接了
//! **另一份** core，`IndexEntry` 等类型与 lib 内的不是同一个类型。`entry` 因此在本模块与
//! `market.rs` 的单测里各有一份——不是疏漏，是那个类型隔断的必然结果。）

use std::collections::BTreeMap;
use std::sync::Mutex;

use agentpipeline_core::agent::market::{
    self, sha256_hex, Downloaded, IndexEntry, MarketClient, KIND_NETWORK,
};
use agentpipeline_core::error::{Error, Result};
use futures::future::BoxFuture;

/// 脚本化的市场客户端：索引与下载结果都由测试给定，**不碰网络**。
#[derive(Debug)]
pub struct FakeMarket {
    index: Result<Vec<IndexEntry>>,
    downloads: Mutex<BTreeMap<String, Result<Downloaded>>>,
}

impl FakeMarket {
    /// 索引拉取直接报错（驱动「索引畸形」与「网络失败发生在索引阶段」两条路径）。
    pub fn index_fails(err: Error) -> Self {
        FakeMarket {
            index: Err(err),
            downloads: Mutex::new(BTreeMap::new()),
        }
    }

    /// 索引畸形（`market_index_malformed`）。
    pub fn malformed_index(raw: &str) -> Self {
        Self::index_fails(Error::Market {
            kind: market::KIND_INDEX.into(),
            message: "技能索引不是合法 JSON".into(),
            raw: raw.into(),
        })
    }

    /// 拉索引时网络失败（`market_network`）。
    pub fn index_unreachable() -> Self {
        Self::index_fails(Error::Market {
            kind: KIND_NETWORK.into(),
            message: "技能市场网络失败（连不上）：拉取技能索引".into(),
            raw: "connect refused".into(),
        })
    }

    /// 提供一份索引。
    pub fn with_entries(entries: Vec<IndexEntry>) -> Self {
        FakeMarket {
            index: Ok(entries),
            downloads: Mutex::new(BTreeMap::new()),
        }
    }

    /// 让某个地址返回给定字节（摘要按内容现算，即「诚实传输」）。
    ///
    /// 摘要与内容一致，故摘要校验这一关会通过——摘要**不符**的场景由索引侧造假
    /// （见 [`IndexEntry::sha256`] 直接改成别的值），比伪造传输层更贴近真实攻击面。
    pub fn serving(self, url: &str, bytes: &[u8]) -> Self {
        self.downloads.lock().unwrap().insert(
            url.to_string(),
            Ok(Downloaded {
                bytes: bytes.to_vec(),
                sha256: sha256_hex(bytes),
                source: market::origin_of(url).unwrap_or_default(),
            }),
        );
        self
    }

    /// 让某个地址返回给定字节，并由 `source` 如实上报**字节的实际来源**（模拟重定向后的结果）。
    ///
    /// 这是「请求 URL 放行、字节来自别处」的对峙局面：白名单只看请求 URL 就会被绕过，
    /// 故 core 侧对 `Downloaded.source` 另有一道独立判定。
    pub fn serving_from(self, url: &str, bytes: &[u8], source: &str) -> Self {
        self.downloads.lock().unwrap().insert(
            url.to_string(),
            Ok(Downloaded {
                bytes: bytes.to_vec(),
                sha256: sha256_hex(bytes),
                source: source.to_string(),
            }),
        );
        self
    }

    /// 让某个地址下载失败（网络失败）。
    pub fn failing(self, url: &str) -> Self {
        self.downloads.lock().unwrap().insert(
            url.to_string(),
            Err(Error::Market {
                kind: KIND_NETWORK.into(),
                message: "技能市场网络失败（请求超时）：下载技能包".into(),
                raw: "timeout".into(),
            }),
        );
        self
    }

    /// 让某个地址返回 `bytes`，但**传输层谎报**摘要是 `claimed`。
    ///
    /// 这是摘要校验里最要紧的那条攻击面：若实现采信传输层声明的摘要（而不是从 `bytes`
    /// 现算），一个被控制的客户端就能同时改内容与声称值，校验形同虚设。索引侧钉住的
    /// `sha256` 是唯一权威——本函数造的就是「传输说谎、索引说真话」的对峙局面。
    pub fn serving_with_lying_digest(self, url: &str, bytes: &[u8], claimed: &str) -> Self {
        self.downloads.lock().unwrap().insert(
            url.to_string(),
            Ok(Downloaded {
                bytes: bytes.to_vec(),
                sha256: claimed.to_string(),
                source: market::origin_of(url).unwrap_or_default(),
            }),
        );
        self
    }

    /// 下载任意**未显式配置**的地址时统一报网络失败。
    ///
    /// 默认行为是「未配置即网络失败」而非 panic：fake 不该因为测试写错了 URL 就崩，
    /// 那会掩盖被测代码的真实表现。
    fn download_of(&self, url: &str) -> Result<Downloaded> {
        self.downloads
            .lock()
            .unwrap()
            .get(url)
            .map(|r| match r {
                Ok(d) => Ok(d.clone()),
                Err(e) => Err(clone_market_error(e)),
            })
            .unwrap_or_else(|| {
                Err(Error::Market {
                    kind: KIND_NETWORK.into(),
                    message: format!("fake 未配置该地址：{url}"),
                    raw: url.to_string(),
                })
            })
    }
}

impl MarketClient for FakeMarket {
    fn index(&self) -> BoxFuture<'static, Result<Vec<IndexEntry>>> {
        // 先把结果从借用里取出来：`BoxFuture<'static>` 不能借用 `&self`
        let result = match &self.index {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(clone_market_error(e)),
        };
        Box::pin(async move { result })
    }

    fn download(&self, url: &str) -> BoxFuture<'static, Result<Downloaded>> {
        let result = self.download_of(url);
        Box::pin(async move { result })
    }
}

/// `Error` 不是 `Clone`，而 fake 要能反复吐出同一个错误（同一地址被下多次）。
fn clone_market_error(e: &Error) -> Error {
    match e {
        Error::Market { kind, message, raw } => Error::Market {
            kind: kind.clone(),
            message: message.clone(),
            raw: raw.clone(),
        },
        other => Error::Validation(other.to_string()),
    }
}

/// 造一条索引条目：摘要取自 `bytes`（即「索引与内容一致」的诚实条目）。
pub fn entry(name: &str, source: &str, bytes: &[u8]) -> IndexEntry {
    IndexEntry {
        name: name.into(),
        version: "1.0.0".into(),
        sha256: sha256_hex(bytes),
        source: source.into(),
        description: Some(format!("{name} 的说明")),
        url: format!("{source}/skills/{name}-1.0.0.zip"),
    }
}

/// 一条摘要是合法 64 位十六进制、但**与内容不符**的条目（驱动摘要不符路径）。
pub fn entry_with_wrong_digest(name: &str, source: &str, url: &str) -> IndexEntry {
    IndexEntry {
        name: name.into(),
        version: "1.0.0".into(),
        sha256: "a".repeat(64),
        source: source.into(),
        description: None,
        url: url.into(),
    }
}

/// 把条目序列化成索引 JSON（与 [`market::parse_index`] 认的格式一致）。
pub fn index_json(entries: &[IndexEntry]) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_serves_index_and_bytes_without_network() {
        let bytes = b"PK\x03\x04 fake zip";
        let e = entry("grill", "https://skills.example.com", bytes);
        let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, bytes);

        let entries = fake.index().await.unwrap();
        assert_eq!(entries.len(), 1);
        let got = fake.download(&e.url).await.unwrap();
        assert_eq!(got.bytes, bytes);
        assert_eq!(got.source, "https://skills.example.com");
        // 诚实传输：声明摘要与现算一致，故能过 `verify_digest`
        assert!(market::verify_digest(&got, &e.sha256).is_ok());
    }

    #[tokio::test]
    async fn unknown_url_reports_network_failure_instead_of_panicking() {
        let fake = FakeMarket::with_entries(vec![]);
        let err = fake
            .download("https://nope.example/x.zip")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Market { kind, .. } if kind == KIND_NETWORK));
    }

    #[test]
    fn index_json_round_trips_through_the_real_parser() {
        let e = entry("grill", "https://skills.example.com", b"body");
        let parsed = market::parse_index(&index_json(std::slice::from_ref(&e))).unwrap();
        assert_eq!(parsed, vec![e]);
    }
}
