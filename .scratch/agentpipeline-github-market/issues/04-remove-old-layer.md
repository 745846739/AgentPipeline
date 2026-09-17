# 04: 拆除旧层（自定 registry 那一层整层退场）

**What to build:** 把"自定 `/index.json` registry"这一层**从代码、界面、测试、文档里删净**，
并确认决策 194 的条款与代码一致。本票是最后一张——它阻塞于票 02 与票 03，正是为了保证
**新层建好并有离线用例之后**才动旧层，不出现"旧层已删、新层没跑通"的不可用窗口。

**Blocked by:** 02、03

**Status:** done（2026-09-17 回填：实现早已落地，随决策 194 于 d41051f 交付）

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

- [x] `rg -n -i "market|index\.json|allowed_sources|MarketClient|FakeMarket"` 的残留**逐条交代**
      （要么已删，要么明确保留并给出理由，例如 0009 的迁移文件要留）
- [x] 旧端点全 404、旧配置键不再被解析（且 `[market] allowed_sources` 出现在既有 `config.toml` 里时
      报错文案要说清它被 `github_repos` 取代——`deny_unknown_fields` 会让它启动即失败，这是正确的
      fail fast 姿态，但文案不能只说"未知字段"）
- [x] 无死代码：`clippy` 无 unused 警告；`docs/testing.md` 的接缝表与 E2E 行与新代码一致
- [x] 全仓闸门：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
      `cargo test --workspace`；前端 `npm run check`、vitest、build；离线 E2E 全绿
- [x] 真 GitHub 的 opt-in 用例在显式开关下仍能跑通（真网络，默认 skip）
- [x] 上表七行逐条核对过，核对结论写回本票

**Notes:** 本仓的历史上"删一层"踩过一次坑（迁移文件不可改，决策 193 记着），所以本票的验收里
"逐条交代残留"比"删干净了"更当作重点——**留下能解释的残留优于悄悄漏掉的残留**。

## 核对结论（2026-09-16，实施后逐条走过）

`rg -n -i "market|index\.json|allowed_sources|MarketClient|FakeMarket"`（排除 `target` / `node_modules` /
`frontend/dist`）的残留逐条交代。**判据是"能不能解释得清"，不是"搜不搜得到"**——旧口径的词在
`docs/decisions.md`（只追加）与术语表的"已退场"条目里**必须**留着，那是消歧件，不属于残留。

| 残留 | 处置 |
|---|---|
| `crates/core/src/storage/migrations/0009_market_sources.sql` | **留文件**（决策 193：已应用过的迁移改了就让既有库启动即报版本不符）。读它的代码全删；表由 0010 的注释点明是别名，不 DROP——清表要一次显式的数据处置决定，不是删文件 |
| `MarketConfig.allowed_sources`（`config.rs`） | **留一个字段，只为报错**：旧键出现在既有 `config.toml` 里时给「已被 `[market] github_repos` 取代（决策 194）」并按旧值回显，而不是 `deny_unknown_fields` 那句「unknown field」。单测 `market_legacy_allowed_sources_key_says_what_replaced_it` 钉住 |
| `crates/core/src/agent/egress.rs` 里两处注释提到 `[market] allowed_sources` | 改口径为 `[market] github_repos`（同一条「回环放行明文 http」的类比） |
| `docs/decisions.md`（172⑤ / 177 / 187 等） | **一个字不改**（只追加）。新口径见决策 194，术语表的"来源白名单（已退场）"条目负责消歧 |
| `docs/glossary.md`、`docs/testing.md`、`docs/implementation.md`、`docs/agents.md`、`docs/operations.md`、`docs/backlog-v2.md` | 同批改口径；术语表新增「已退场」条目，旧词出现时按它消歧 |
| `crates/core/src/agent/market.rs` / `market_sources.rs` / testkit 的 `FakeMarket` / 前端 `marketSources.ts` / `marketRegistry.ts` / `e2e/marketRegistry.ts` | **已删**（前三个是旧层的实现与替身，后三个是旧层的界面校验器与 E2E 装置） |
| `Makefile` 里 `TESTS=market` 的例子 | 留：那是**新** `market.rs`（本票之后的契约用例文件），不是旧层 |
| `.scratch/agentpipeline-v2-skills/issues/10-remote-registry.md` | 留：历史票面，记录的是当时的做法；由决策 194 与票 04 的删单指向它 |

旧端点守卫：`crates/app/tests/market.rs` 新增 `the_retired_endpoints_are_gone`——`/market/config`
（GET/PUT/DELETE）、`/market/search`、`/market/index`、`/market/index.json` **全部 404**。

**一处与票面不同的实现**：`POST /skills/install` 上"这个名字不在推荐清单里"从 400 改成
**404 + `kind = skill_not_found`**。理由：票 02 明写"决策 181 的失败映射要同批改"（不搬就会变成
"八类里有两类永远映射不到、界面按四类分支"），而这个端点上"装不上这个技能"与"这个仓里没有那个目录"
对用户是同一句话（动作同为**换技能**），复用同一个 `kind` 比自造第五种干净。

**仍未验的一条**（别当已验）：`repo_unreadable` 的可判定性——无凭据读私有仓时 GitHub 回 401 系列
还是干脆 404，没测（要真私有仓）。`is_auth_shaped` 因此可能永远打不到，该类会退化成
`repo_not_found`；两类文案里**都已经写了「也可能是私有仓且无权访问」**，所以两条路都不会把用户
引错方向。这条写在 `repo.rs` 的 `unreadable` 文档里。

---

## 实施收尾（2026-09-16）

验收全过。闸门读数：`cargo fmt --all -- --check` 干净、`clippy --workspace --all-targets -- -D warnings`
干净、`cargo test --workspace` **全绿**（app 契约 104 + 技能来源 25 + core 448 单测等）；
前端 vitest **350**、`svelte-check` 0 错 0 警告、`vite build` 通过；Playwright 全量
**43 passed / 2 skipped**（跳过的是"截图作为证据"那两条）。真 GitHub 冒烟在显式开关下**实测一轮通过**。

## 代码评审（两轴，2026-09-16）

按 `code-review` 的两轴各跑一遍（Standards / Spec），**发现的问题逐条修掉**，留档：

| 轴 | 发现 | 处置 |
|---|---|---|
| Spec | `digest_mismatch` 八类里唯一**没有任何代码路径能产生**的一类（常量、文案、界面提示都在，构造器不存在） | 补 `is_digest_shaped`（`ErrorClass::Sha1` 或 `hash mismatch` / `checksum` / `corrupt` / `invalid object`）+ `digest_mismatch` 构造器，**并前置到"对象不在本地就是取不到"那条判定之前**（两者都会让 `find_commit` 找不到对象，顺序错了会把"别装、报警"报成"换一个 commit"）。两条单测钉住 class / 措辞与归类。**离线 fixture 造不出这个失败**（要手搓一个哈希坏掉的 pack），如实记账 |
| Spec | `egress.rs` 两处注释仍写着 `[market] allowed_sources`（票 04 的删单明写"引用要改"，而核对结论当时**错记成已改**） | 改成 `[market] github_repos`（那条"回环放行明文 http"的类比与决策 194 同源） |
| Spec | 界面按**报文里的字样**判同名冲突（`/已存在/.test(message)`），与"按 `kind` 分支、不按字样"的口径相悖 | 改按 **409** 判：这个端点上 409 只有一个含义，是状态码里没有歧义的那一种（八类要求的"别按状态码"针对的是 404 / 400 上各挤着好几类） |
| Spec | 迁移 0011 的注释还写着已放弃的规则（"要比 commit、一致才跳过"） | 注释改成实现的规则（仓 + 子路径，**不比 commit**；没有记录算一致——那正是决策 181⑦ 的离线路径）。**该文件本批新增、从未进过任何既有库**，故不触决策 193 |
| Spec | `docs/testing.md` 的 194 行把 §7 契约指向 `api_contract.rs`，而市场段已搬到 `crates/app/tests/market.rs` | 行内改指向，并把两处覆盖损失（`repo_unreadable` 判不出来 / `digest_mismatch` 造不出来）写进同一行 |
| Standards | `record_skill_source` 把一个公开字段 **丢了**：SQL 里 bind 的是 `ts(self.now())` 而不是调用方传进来的 `source.installed_at`（两个调用点都认真填了它） | 改成 bind 传进来的值 |
| Standards | 未启动时的兜底实现（`Libgit2Repo::default()`）把缓存放系统临时目录，而 `serve.rs` 的注释正说着"落 /tmp 会被清理器删掉" | 生产一直走 `with_repo(home/market-repos)`；给 `Default` 补注释说明它只是给 `AppState::new` 的初值、**生产必须注入**，免得后人照它写 |
| Standards | `matches_exact` / `store.skill_sources_for` **无人调用**，且后者的文档说"一次查完"而实现是循环单查（N+1） | 一并删掉。`matches_exact` 是被"跳过判据不比 commit"那条裁决变成死代码的——评审抓到的正是这处**裁决与残留代码的时差** |
| Standards | 一键安装 `note` 里那处字符串续行漏了反斜杠，报出一串字面空格 | 补回 `\` 续行 |
| Standards | 前端 `marketRepos.ts` 与 `RepoId` 的"同口径"说法过于含糊（它其实**先归一再校验**，`www.` 前缀是靠归一抹掉的） | 注释改成有方向的表述：不变式是「归一的输出必须是 `RepoId` 接受的输入的子集」 |

**未改的（判断为主，写明理由）**：
- 两个安装入口（`/market/install` 与一键安装）确实重复了 `repo_allowed → 取 → install → 冲突报文 → 记来源`
  这条管线。抽公共函数是日后的清理项，本批不动——两边的形状不同（一边拿 `owner/repo/commit`，
  一边按清单定位），而**判定与措辞已经共用**（`repo_allowed` 与 `conflict_with_origin` 各只有一份），
  今天没有第二份口径。
- 仓名**语法非法**时报 `repo_not_allowed`（不是 `repo_not_found`）：报文点名了"仓名不合法"并回显原值，
  用户的动作（改写成合法 `owner/repo`）不会被引偏；为它单造一类不值得。
