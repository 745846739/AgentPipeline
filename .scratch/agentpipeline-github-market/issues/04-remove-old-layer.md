# 04: 拆除旧层（自定 registry 那一层整层退场）

**What to build:** 把"自定 `/index.json` registry"这一层**从代码、界面、测试、文档里删净**，
并确认决策 194 的条款与代码一致。本票是最后一张——它阻塞于票 02 与票 03，正是为了保证
**新层建好并有离线用例之后**才动旧层，不出现"旧层已删、新层没跑通"的不可用窗口。

**Blocked by:** 02、03

**Status:** ready-for-agent

> "整层替换"说的是**最终形态不留两套**，不是指中间不能有顺序。并存被否决的理由是：它会把"放行判定"
> 与"失败分类"各变成两份实现，而放行判定是本系统唯一的安全控制（决策 187 的原话是"这条判定不能有
> 第二个版本"）。

## 删的清单（文件级）

**后端**

| 位置 | 处置 |
|---|---|
| `crates/core/src/agent/market.rs` | 整文件退场（`IndexEntry` / `Downloaded` / `MarketClient` / `search` / `parse_index` / `pick_entry` / `source_allowed` / `verify_digest` / `install_from_market` / `HttpMarketClient` / `origin_of`）。**但 `sha256_hex` 这类工具函数要先查调用方**——它可能被市场之外的地方复用（配对令牌、口令一类的路径），别跟着整文件一起删 |
| `crates/core/src/config.rs` | `MarketConfig.allowed_sources` → `github_repos`；`validate_market_sources` → 仓名单的校验（与界面共用同一函数，与票 01 的 `RepoId` 判定是同一处）；解析期 fail fast 那两处（`config.rs:477` 起）跟着改；`normalize_origin` 若只被市场用则一并退场 |
| `crates/core/src/storage/market_sources.rs` | 退场 |
| `crates/core/src/storage/migrations/0009_market_sources.sql` | **保留文件本身，只删读写它的代码**（见下"一个不能碰的东西"） |
| `crates/app/src/routes/market.rs` | 旧三组端点（`/market/search`、`/market/install`、`/market/config` 的 GET/PUT/DELETE）退场，由票 02 / 03 的新端点接替 |
| `crates/app/src/state.rs`、`serve.rs`、`lib.rs` | `AppState.market: Option<Arc<dyn MarketClient>>` 及其注入退场，换成票 01 的接缝 |
| `crates/core/src/agent/egress.rs` | **姿态不动，引用要改**：`:28` 与 `:504` 都写着"与 `[market] allowed_sources` 同姿态"，注释不能继续指向一个不存在的配置项 |
| `crates/testkit/src/market_fixture.rs` | `FakeMarket` 退场，由票 01 的两个 fixture（本地裸仓 / 离线 smart HTTP）接替 |
| `crates/core/tests/market.rs`、`crates/app/tests/api_contract.rs` | 市场部分的用例整批退场/改写（后者 24 处） |

**前端**

`SettingsMarket.svelte`（来源白名单那半）、`lib/marketSources.ts` + `marketSources.test.ts`、
`api/client.ts` 与 `api/types.ts` 里的市场类型、`e2e/marketRegistry.ts`（手搓 ZIP 的离线 registry）、
`e2e/market.spec.ts`（E2E ⑫）与 `e2e/market-install.spec.ts`（E2E ⑬）两份用例。
`vite.config.ts` / `router.svelte.ts` / `router.test.ts` / `App.svelte` / `TopBar.svelte` 里的引用逐条核对。

**离线 E2E 的换血方向**：⑬ 钉的是"安装通路本身"，这个价值保留，但它的 fixture 从"手搓 ZIP + 普通 HTTP"
换成**离线 smart HTTP 的 git 仓**（票 01 的那一套）——于是它顺带把 shallow 与传输策略也打了，
这是旧 fixture 打不到的。⑫ 的旧断言（"白名单是空的 / 不允许远程安装"）按新语义重写。**两张都保留，
不要合并成一张**：⑫ 钉"界面与配置的两级关系"，⑬ 钉"装得下来、落得对"，是两个独立失效面。

**文档**

`docs/agents.md`（"从远程 registry 安装"整节）、`docs/glossary.md`（技能市场词条）、
`docs/implementation.md`（§11 的端点表与**第五条接缝那一行**）、`docs/operations.md`（出网/威胁模型里
提到市场来源的那处）、`docs/testing.md`（用例目录、五条可测试性接缝表、E2E ⑫⑬ 两行）。
`AGENTS.md` 里"五条可测试性接缝（决策 143，由决策 177 修订为五条）"要改成由**决策 194** 修订**形状**
（条数仍是五条，第五条从"网络出口加一条 `MarketClient`"变成"仓访问加一条 `SkillRepo`"）。

## 一个不能碰的东西（这条有前例）

**迁移 `0009_market_sources.sql` 的文件本身保留，只删读写它的代码。** 理由与决策 193 里记的那条同一性质：
`sqlx::migrate!` 对每个已应用过的迁移文件记校验和，**改动或删除已应用的迁移都会让既有库在启动时报版本
不符**。所以表可以变成没人读的遗留表，文件不能消失。若要连表一起清掉，那是**一条新迁移**
（`DROP TABLE`）加一次显式的数据处置决定，不是本票的顺手事。

## 决策 194 的生效确认

决策 194 的正文里逐条写着"177②/③ 作废，改由本决策承担"式的修订关系。**本票是让它与代码一致的那一张**：
落地后按下面这张表逐条核对，并把核对结论写回票里（哪几条已由代码承担、哪几条只是文档层面的替换）。

| 被修订的 | 旧条款 | 新承担者 |
|---|---|---|
| 决策 172⑤ | 自定 index 格式 + 来源 origin 白名单 | 票 01 / 02 / 03 |
| 决策 177② | 不跟随 HTTP 重定向（`Policy::none()`） | 票 01：`RemoteRedirect::None` **显式设**，且注释写明它只是"不跟跨站"（同站 http→https 升级仍放行，对我们不可达） |
| 决策 177③ | 非回环来源必须 https | 票 01：URL 只能由我们构造，形态只有 `https://github.com/{owner}/{repo}.git` 一种 |
| 决策 177 摘要口径 | `sha256` 的权威值从下载字节现算 | 票 01 / 02：权威值是 **commit SHA**，由 libgit2 在 fetch 时本地校验对象哈希；`market_digest_mismatch` 语义升级为"对象哈希不符" |
| 决策 177 五类失败 | network / not_found / digest_mismatch / source_not_allowed / index_malformed | 票 02 的八类表（判据不变：每类一个互不相同的用户动作） |
| 决策 187 | 界面那份**来源名单** + `validate_market_sources` | 票 03：`owner/repo` 仓名单 + 与 `RepoId` 共用的校验；两级结构（保存即生效 / 清掉回配置）**继承** |
| 决策 181⑤⑦ | 失败映射复用票 10 的四类；推荐清单只有名字 + 理由 | 票 02：八类映射 + 清单带定位字段 + "已装即跳过下载"分支按 commit 是否一致重写 |

## 验收

- [ ] `rg -n -i "market|index\.json|allowed_sources|MarketClient|FakeMarket"` 的残留**逐条交代**
      （要么已删，要么明确保留并给出理由，例如 0009 的迁移文件要留）
- [ ] 旧端点全 404、旧配置键不再被解析（且 `[market] allowed_sources` 出现在既有 `config.toml` 里时
      报错文案要说清它被 `github_repos` 取代——`deny_unknown_fields` 会让它启动即失败，这是正确的
      fail fast 姿态，但文案不能只说"未知字段"）
- [ ] 无死代码：`clippy` 无 unused 警告；`docs/testing.md` 的接缝表与 E2E 行与新代码一致
- [ ] 全仓闸门：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
      `cargo test --workspace`；前端 `npm run check`、vitest、build；离线 E2E 全绿
- [ ] 真 GitHub 的 opt-in 用例在显式开关下仍能跑通（真网络，默认 skip）
- [ ] 上表七行逐条核对过，核对结论写回本票

**Notes:** 本仓的历史上"删一层"踩过一次坑（迁移文件不可改，决策 193 记着），所以本票的验收里
"逐条交代残留"比"删干净了"更当作重点——**留下能解释的残留优于悄悄漏掉的残留**。
