//! 「什么算本机」的唯一实现（决策 246）——四处判回环里的 **Rust 那一处**。
//!
//! 四处判回环：出口策略放不放行（`agent::egress`）、技能来源仓要不要 https（`agent::repo`）、
//! 服务绑定与配对令牌（`app` 的 `peer`）、前端页面自己开在哪儿（`frontend/src/lib/localPage.ts`）。
//! 前三处调本模块，前端**调不了 Rust**（页面 hostname 的判定可能发生在任何 API 调用之前），
//! 故同源靠 [`tests/fixtures/host_policy_loopback.json`] 这张共享表——本模块的表测试与
//! 前端 vitest 读同一份、同一断言方向，一侧改了规范另一侧没跟就变红。
//!
//! ## 判定（决策 246 的规范形态）
//!
//! 归一（去空白 / 转小写 / 去一个尾点 / 脱方括号）→ 能 `parse::<IpAddr>` 的走 `is_loopback()` →
//! 否则判 `localhost`。
//!
//! **不按 `127.` 前缀匹配**：`127.evil.test` 是外部域名（DNS 可解析到攻击者自己的机器），
//! 前缀匹配会把这类伪装放行成回环豁免——决策 179② 的 `127.*` 字面写法正是这样被
//! `egress.rs` 原样实现的，而决策 194 在 `repo.rs` 里已经改成 `IpAddr` 读法却没回传：
//! 两份正确性不同的实现并存，本模块就是那次传播失败的收口。能到回环的 IPv4-mapped 形态
//! （`::ffff:127.0.0.1`）也拒——按「错的方向是多拦」的偏向不为它开特例。
//!
//! ## 不在判定域内的一处
//!
//! `app::peer::PeerAddr::is_loopback` 判的是 axum 解析好的 `SocketAddr` 的 IP
//! （输入已经是 `IpAddr`，不是主机名字符串），按决策 246 的「明确不做」原样不动。

/// 回环判定：归一 → IP 字面量 `is_loopback()` → 否则 `localhost`（决策 246）。
///
/// interface 只有这一个函数；归一是实现细节，不进 interface。
pub fn is_loopback(raw: &str) -> bool {
    let host = normalize(raw);
    if host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        return true;
    }
    host == "localhost"
}

/// 归一：去空白 / 转小写 / 去一个尾点 / 脱方括号（幂等；实现细节，见模块头）。
fn normalize(raw: &str) -> String {
    let lowered = raw.trim().to_lowercase();
    let without_dot = lowered.strip_suffix('.').unwrap_or(&lowered);
    if let Some(rest) = without_dot.strip_prefix('[') {
        // `[::1]` / `[::1]:8080` 形态：URL 的 IPv6 hostname 带方括号
        return rest.split(']').next().unwrap_or("").to_string();
    }
    without_dot.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跨语言共享表（票 02）：输入 → 期望逐行断言，覆盖决策 246 的全输入表。
    ///
    /// 这张表同时是前端 `localPage.test.ts` 的断言源——同一份数据、同一断言方向，
    /// 「Rust 改了规范、前端没跟」从此有一个会变红的落点（fixture 头注写明了两侧口径）。
    /// 决策 194 在 `repo.rs` 钉过的 evil-input 断言（`127.0.0.1.evil.test` 不算回环）
    /// 随谓词迁入本表，由 fixture 行 `127.0.0.1.evil.test → false` 承接。
    #[test]
    fn shared_fixture_covers_the_whole_decision_246_input_table() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            cases: Vec<Case>,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            input: String,
            loopback: bool,
        }

        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/host_policy_loopback.json"
        ));
        let fixture: Fixture = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");
        assert!(
            fixture.cases.len() >= 20,
            "全输入表不该被悄悄裁短：{}",
            fixture.cases.len()
        );
        // 形状守卫（与 vitest 侧同一把尺）：放行 / 拒绝两侧都非空——
        // 退化成全 `true` 时逐行断言照样绿，那等于没测。
        assert!(
            fixture.cases.iter().any(|c| c.loopback),
            "放行侧一行都不剩 = 表退化了"
        );
        assert!(
            fixture.cases.iter().any(|c| !c.loopback),
            "拒绝侧一行都不剩 = 表退化了"
        );
        // 必需行**按名**钉住：只数行数时，把 `::ffff:127.0.0.1` 换成任意多余行也能过 ≥20。
        for required in [
            "localhost.",
            "LOCALHOST",
            "127.0.0.1.evil.test",
            "127.999.999.999",
            "0.0.0.0",
            "::ffff:127.0.0.1",
            "[::1].",
        ] {
            assert!(
                fixture.cases.iter().any(|c| c.input == required),
                "缺必需行 {required:?}"
            );
        }
        for case in &fixture.cases {
            assert_eq!(
                is_loopback(&case.input),
                case.loopback,
                "input = {:?}",
                case.input
            );
        }
    }

    /// 归一是幂等的：消费者有的喂原始输入、有的喂已归一的副本，答案必须一致。
    #[test]
    fn normalize_is_idempotent() {
        for raw in [" LocalHost ", "[::1]", "127.0.0.1."] {
            let once = normalize(raw);
            assert_eq!(normalize(&once), once, "{raw:?}");
        }
    }
}
