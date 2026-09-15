//! L2 集成：从远程 registry 安装技能（决策 172⑤，票 10）。
//!
//! 这几条用例要驱动 [`install_from_market`] 的完整路径（索引 → 白名单 → 下载 → 摘要 →
//! 落盘），需要 **testkit 的 fake 市场客户端**。fake 住在 testkit，而 testkit 经
//! dev-dependency 链接了另一份 core——它的 `IndexEntry` 与 lib 内的不是同一个类型，
//! 因此这些用例只能待在 integration 层（体例同 `executor.rs` 用 `FakeAgent`）。
//!
//! 纯函数（索引解析 / 筛选 / 白名单 / 摘要）的用例仍在 `src/agent/market.rs` 的
//! `mod tests` 里，不依赖 fake。**全部不打真网络**。

use agentpipeline_core::agent::market::{
    install_from_market, KIND_DIGEST, KIND_INDEX, KIND_NETWORK, KIND_NOT_FOUND, KIND_SOURCE,
};
use agentpipeline_core::Error;
use testkit::{entry, FakeMarket};

const SOURCE: &str = "https://skills.example.com";

fn allowed() -> Vec<String> {
    vec![SOURCE.to_string()]
}

/// 造一个单技能 zip（`{name}/SKILL.md`）。
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

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// 从 Error 里取市场失败类别（复用 `Error` 上的访问器，不在测试里另写一遍 match）。
fn market_kind(err: &Error) -> Option<&str> {
    err.market_kind().map(|(kind, _)| kind)
}

// ── 端到端（fake 驱动，五条路径，全部不打真网络）──

#[tokio::test]
async fn market_install_lands_the_skill() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = write_zip("grill", "拷问协议正文");
    let e = entry("grill", SOURCE, &bytes);
    let client = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);

    let info = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap();
    assert_eq!(info.name, "grill");
    // 落盘交给票 09，布局与本地导入完全一致
    assert!(root.join("grill/SKILL.md").is_file());
    assert!(agentpipeline_core::agent::skills::skill_names(&root).contains(&"grill".to_string()));
}

/// 路径②：摘要不符 → 拒绝安装，且**不落盘**。
#[tokio::test]
async fn market_install_rejects_digest_mismatch() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = write_zip("grill", "正文");
    // 索引声明一个与真实内容不符的摘要（模拟篡改 / 索引过期）
    let mut e = entry("grill", SOURCE, &bytes);
    e.sha256 = "a".repeat(64);
    let client = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);

    let err = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_DIGEST), "{err:?}");
    assert!(!root.join("grill").exists(), "摘要不符不得落盘");
}

/// 路径③：来源未放行 → 拒绝（且在下载之前）。
#[tokio::test]
async fn market_install_rejects_unallowed_source() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = write_zip("grill", "正文");
    let e = entry("grill", SOURCE, &bytes);
    let client = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);

    let err = install_from_market(&client, &root, "grill", &[], false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_SOURCE), "{err:?}");
    assert!(!root.join("grill").exists());

    // 索引放行但**下载地址**指向别处 → 同样拒绝（CDN 指向是白名单要拦的点）
    let mut redirect = e.clone();
    redirect.url = "https://cdn.evil.example/grill.zip".into();
    let client2 = FakeMarket::with_entries(vec![redirect.clone()]).serving(&redirect.url, &bytes);
    let err = install_from_market(&client2, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_SOURCE), "{err:?}");
}

/// 路径④：索引畸形 → 拒绝（不与网络失败混淆）。
#[tokio::test]
async fn market_install_reports_malformed_index() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let client = FakeMarket::malformed_index("<html>404</html>");
    let err = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_INDEX), "{err:?}");
}

/// 路径⑤：网络失败 → 可归因的网络错误，**与摘要不符 / 来源未放行严格区分**。
#[tokio::test]
async fn market_install_reports_network_failure_distinctly() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = write_zip("grill", "正文");
    let e = entry("grill", SOURCE, &bytes);
    let client = FakeMarket::with_entries(vec![e.clone()]).failing(&e.url);

    let err = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_NETWORK), "{err:?}");
    // 三类失败在报文上也必须分得开
    let msg = err.to_string();
    assert!(msg.contains("网络"), "{msg}");
    assert!(!msg.contains("摘要不符"), "不得与摘要不符混淆：{msg}");
    assert!(!msg.contains("未放行"), "不得与来源未放行混淆：{msg}");
}

#[tokio::test]
async fn unknown_skill_in_index_is_not_found() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = write_zip("grill", "正文");
    let e = entry("grill", SOURCE, &bytes);
    let client = FakeMarket::with_entries(vec![e.clone()]);

    let err = install_from_market(&client, &root, "nope", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_NOT_FOUND), "{err:?}");
    assert!(err.to_string().contains("nope"), "{err}");
}

/// 票 09 的防护在市场路径上同样生效：穿越的包即使摘要对得上也被拒。
///
/// 这是「落盘只实现一次」的价值——远程包不许比本地上传的包享有更宽的路。
#[tokio::test]
async fn market_package_still_undergoes_traversal_defense() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    // 恶意包：摘要合法（索引与内容一致），但条目名穿越
    let evil = {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::SimpleFileOptions = Default::default();
            w.start_file("s/SKILL.md", opts).unwrap();
            w.write_all("---\nname: s\n---\n\n正文".as_bytes()).unwrap();
            w.start_file("../escaped.md", opts).unwrap();
            w.write_all("逃逸".as_bytes()).unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    };
    let e = entry("s", SOURCE, &evil);
    let client = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &evil);

    let err = install_from_market(&client, &root, "s", &allowed(), false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("穿越"), "{err:?}");
    assert!(!home.path().join("escaped.md").exists());
    assert!(!root.join("escaped.md").exists());
}

/// 同名的本地技能已存在时，市场安装走票 09 的同名冲突（默认拒绝）。
#[tokio::test]
async fn market_install_respects_existing_local_skill() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(root.join("grill")).unwrap();
    std::fs::write(root.join("grill/SKILL.md"), "本地已有的正文").unwrap();

    let bytes = write_zip("grill", "市场来的正文");
    let e = entry("grill", SOURCE, &bytes);
    let client = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);

    let err = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    assert!(std::fs::read_to_string(root.join("grill/SKILL.md"))
        .unwrap()
        .contains("本地已有的正文"));
}

/// 摘要的**权威值来自 bytes 现算**，不采信传输层声明。
///
/// 攻击面：被控制的客户端同时改内容与声称值。若实现采信 `Downloaded.sha256`，
/// 索引里钉住的摘要就形同虚设——这个用例让传输层撒谎（声称索引里那个摘要），
/// 而内容其实是别的字节，必须被拒。
#[tokio::test]
async fn transport_digest_is_never_trusted_over_the_computed_one() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();

    let honest = write_zip("grill", "索引里对应的正文");
    let e = entry("grill", SOURCE, &honest);
    // 索引钉住诚实内容的摘要……
    let expected = e.sha256.clone();
    // ……但传输层送来的是**另一些字节**，同时谎称摘要就是索引里那个
    let swapped = write_zip("grill", "被换掉的正文");
    let client = FakeMarket::with_entries(vec![e.clone()])
        .serving_with_lying_digest(&e.url, &swapped, &expected);

    let err = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_DIGEST), "{err:?}");
    assert!(!root.join("grill").exists(), "换过内容的包不得落盘");
}

/// **字节实际来源**也须在白名单内：请求的 URL 放行 ≠ 字节来自那里。
///
/// 攻击面：一个**已放行**的来源回 302，把内容指到任意别处（内网元数据端点之类）。白名单若
/// 只看请求 URL 就当场失效。`HttpMarketClient` 侧已关掉重定向跟随（`Policy::none()`），
/// 本用例钉住 core 侧那道**独立**判定——即使某个客户端跟随了重定向并如实上报最终来源，
/// 安装也必须拒绝。
#[tokio::test]
async fn redirected_origin_is_rejected_even_when_the_requested_url_is_allowed() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();

    let bytes = write_zip("grill", "正文");
    let e = entry("grill", SOURCE, &bytes);
    // 请求地址在 SOURCE 白名单内，但字节实际来自别处
    let client = FakeMarket::with_entries(vec![e.clone()]).serving_from(
        &e.url,
        &bytes,
        "https://169.254.169.254",
    );

    let err = install_from_market(&client, &root, "grill", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_SOURCE), "{err:?}");
    assert!(!root.join("grill").exists(), "重定向到未放行来源不得落盘");
}

/// 索引里的名字若不可能落盘（含路径分隔符），须在**下载之前**就拒绝。
///
/// 否则一个 `a/b` 这类名字会先烧掉一次下载，再在落盘时才被票 09 的名字不变量拒掉——
/// 与「摘要格式早校验」同一理由。
#[tokio::test]
async fn unusable_index_name_is_rejected_before_downloading() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();

    let bytes = write_zip("s", "正文");
    let mut e = entry("s", SOURCE, &bytes);
    e.name = "a/b".into();
    // fake 里**不配置**该地址 → 若真的去下载，会得到一条网络失败而不是索引错误。
    // 断言拿到的是 KIND_INDEX，即证明这一步早于下载。
    let client = FakeMarket::with_entries(vec![e.clone()]);

    let err = install_from_market(&client, &root, "a/b", &allowed(), false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_INDEX), "{err:?}");
    assert!(err.to_string().contains("路径分隔符"), "{err}");
}

/// 空白名单时**在拉索引之前**就拒绝（默认拒绝是结构性质，不是靠上层记得别建客户端）。
#[tokio::test]
async fn empty_allowlist_is_refused_before_any_network_call() {
    let home = tmp();
    let root = home.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();

    // 索引构造为「必失败」：若实现先去拉索引，拿到的会是网络失败而非来源错误
    let client = FakeMarket::index_unreachable();
    let err = install_from_market(&client, &root, "grill", &[], false)
        .await
        .unwrap_err();
    assert_eq!(market_kind(&err), Some(KIND_SOURCE), "{err:?}");
}
