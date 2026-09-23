# 23: RepoId 前后端两份判定收口 + 两个未钉住的 failure kind

**What to build:** 两件同源的事（「文档声称 ≠ 代码事实」的同一种病，架构评审候选 4 拷问时
裁定只记录、另立一票——决策 250 Q2）：

1. **`RepoId` 合法性判定收成一处。** `docs/glossary.md` 技能来源仓词条与决策 187 原话都
   声称「这条判定只有一处实现，界面上不另写正则」，而 `frontend/src/lib/marketRepos.ts` 的
   `validateRepo` / `normalizeRepo` 是一份 27 行的**重新实现**，自带正则
   （`/^[A-Za-z0-9._-]+$/`），且 `normalizeRepo` 刻意放宽了归一（剥 `www.github.com` 前缀——
   后端 `RepoId::parse` 只剥 `github.com` 那族、不认 `www.`）。文件头自己承认不变量只是
   「**前端输出 ⊆ 后端接受集**」。要收口：子集关系必须**被机器钉住**（不是注释里的承诺），
   而不是把前端变成调后端的壳（页面 hostname 判定可能发生在任何 API 调用之前，
   照决策 246 `localPage` / 共享 fixture 的先例走同源断言，或另有更优形状——实现者裁）。
2. **补两个 API 层未钉住的 failure kind。** 决策 194 裁决⑦ 承诺八类失败「互不混淆」，
   当前 `crates/app/tests/integration/market.rs` 只断言了 6/8：缺
   `repo_unreadable`（生产在 `repo.rs:81` / 401·404 分类）与 `digest_mismatch`
   （`repo.rs:83,1090`，git 对象哈希不符 400）的 **kind 断言**——两者生产代码都在、
   core 层有单测，但端点契约层没有用例钉「界面按 kind 分支时这一类拿得到」。

**Blocked by:** 无（决策 250 已落档，本票是它 Q2 的「另立一票」）

**Status:** done

## 一、RepoId 收口

- [x] 现状取证：前端 `validateRepo`/`normalizeRepo` 与后端 `RepoId::parse` 的规则对照
      （机读断言是 `tests/fixtures/repo_id.json` 的 34 行——它钉的是**归一后形态**上的
      子集不变量；Rust 侧断言的对象本来就是过网的 `normalized`，故只在**原始输入**上
      单侧存在的分歧（下面第 2、3 条）按 prose 记录、不入表——那不是不变量的约束对象。
      逐条结论：）
      - **两边语义相同**：恰好一段 `/`、两段非空、字符集 `[A-Za-z0-9._-]`、不以 `.`/`-`
        开头、拒 `..`、拒非 ASCII、trim、抹 `github.com/` 粘贴前缀、去 `.git`、大小写
        照收不改；
      - **只有一边有、但不破子集不变量**：
        - `www.github.com/` 前缀剥离**前端独有**（后端只认 `github.com` 那族）——前端
          先剥，过网时该前缀已不在；fixture 的 `www` 行钉住这一步；
        - `.git` 与尾斜杠的**剥离次序**不同：前端先去尾斜杠再去 `.git`（`a/b.git/` →
          `a/b`），后端先去 `.git` 再去尾斜杠（原始输入 `a/b.git/` 会被收成字面名
          `a/b.git`）——分歧只在**原始输入**上；Rust 侧断言的是**归一后**的形态，
          前端输出 `a/b` 后端照收；
        - 前端有显式「带 `://`」拒绝（为报错文案），后端靠分段结构天然拒（`://` 自带
          两个 `/`，必落多余段或空段）——判定出口相同；
      - **没有**「前端放行、后端拒绝」的条目——这正是被钉住的子集不变量本身。
- [x] 裁定形状：**a. 共享 fixture 表**（照决策 246 `tests/fixtures/host_policy_loopback.json`
      先例）——b 被否（API 调用时机问题，与 246 同一个理由），c 不需要（a 已把不变量
      变成会变红的测试）。
      - 表：`tests/fixtures/repo_id.json`（34 行 `{input, normalized, valid}`）；
      - vitest 侧 `frontend/src/lib/marketReposFixture.test.ts`：钉前端逐行产出
        `normalized`/`valid`；
      - Rust 侧 `repo.rs::shared_repo_id_fixture_pins_frontend_output_inside_backend_accepts`：
        钉**表里的 `normalized` 就是 `RepoId::parse` 认识的输入**（成功 ⟺ `valid`，且
        `slug()` 原样）；
      - 两侧同表、同一断言方向，任何一侧漂了另一侧没跟就变红；**反向验证做过**：
        把表里 `valid` 翻转 → 两侧同时红 → 还原。
- [x] 收口后更新两处口径：`glossary.md` 技能来源仓词条（「只有一处实现 / 界面上不另写
      正则」→「**规范只有一处**、实现两侧、子集不变量由共享表机器钉住」）与
      `marketRepos.ts` 文件头（子集不变量从「这条注释存在的意义」升成共享表 + 两侧测试）。

## 二、补两个 failure kind 的契约断言

- [x] `repo_unreadable`：`market.rs::auth_required_remote_is_reported_as_repo_unreadable`
      ——testkit `SmartHttp` 新增 `RemoteBehaviour::AuthRequired`（一切请求回 401，即「私有仓、
      无凭据」的远端形态），断言 404 + `kind == repo_unreadable` + 文案点到「私有仓 /
      换公开仓」。真 GitHub 回 401 还是 404 的实测缺口**仍开着**（票 04，`repo.rs::unreadable`
      的注继续记着）——这里钉的是分类路径与契约出口。
- [x] `digest_mismatch`：**取证结论——演得了。** `RemoteBehaviour::CorruptPack` 把
      `git-upload-pack` 响应里的 pack 尾哈希改坏（内容照流、framing 不动），即真传输损坏
      同形；`market.rs::corrupted_pack_is_reported_as_digest_mismatch` 断言 400 +
      `kind == digest_mismatch` + 报文劝住「别装」+ `detail` 单列。
      取证过程记下的两个服务端事实：① pkt-line 的 flush `0000` 长度**值**是 0（特殊值、
      占 4 字节），当畸形会把连接打断；② `depth(1)` 浅取协商**第一轮**只有
      `shallow <sha>` + flush、pack 在下一轮 POST——「本轮无 pack」是正常形态，
      fixture 对此放行、但「有 pack 却走不到」仍 bail（反假绿哨兵）。
      **顺带修了一处生产分类缺口**：实测 libgit2 1.9.7 对尾哈希不符报
      `class=Indexer msg=packfile trailer mismatch`——既非 `Sha1` class、字面也不含
      `hash mismatch`，不入 `is_digest_shaped` 清单就会掉进 `commit_not_found`
      （把「别装、报警」说成「换一个 commit」，方向正好反）；措辞已入清单 + core 单测。
- [x] 八类 → 8/8，逐类钉在哪层：

  | # | kind | 钉在哪层 · 用例（`market.rs` 为 `crates/app/tests/integration/market.rs`） |
  |---|---|---|
  | ① | `market_network` | API 契约 · `network_failure_is_a_bad_gateway_with_its_own_kind` |
  | ② | `repo_not_found` | API 契约 · `failure_classes_are_distinguishable_by_kind` ② |
  | ③ | `commit_not_found` | API 契约 · 同上 ③④（不存在的 SHA + 缩写 SHA 两种入口） |
  | ④ | `skill_not_found` | API 契约 · 同上 ⑤ |
  | ⑤ | `repo_not_allowed` | API 契约 · 同上 ①（kind 断言）；⑥ 是同一处判定的另一入口，但走配置错误映射、**不带 kind**（只钉 400 + 报文，实测记录见实施记录）；core 纯函数单测另有 |
  | ⑥ | `repo_unreadable` | API 契约（**本票新增**）· `auth_required_remote_is_reported_as_repo_unreadable`；分类器 core 单测另钉措辞 |
  | ⑦ | `digest_mismatch` | API 契约（**本票新增**）· `corrupted_pack_is_reported_as_digest_mismatch`；core · `a_digest_failure_maps_to_the_digest_class` + `is_digest_shaped` 措辞表（含实测原话 `packfile trailer mismatch`） |
  | ⑧ | `download_too_large` | API 契约 · `download_over_the_cap_is_reported_with_our_own_count` |

## 验收

- [x] `make check` 绿（决策 168 的唯一权威闸门）——2026-09-23 全量 exit 0
      （fmt + clippy + workspace 测试 + 前端 + e2e）
- [x] `grep -n "子集\|另写正则" docs/glossary.md frontend/src/lib/marketRepos.ts`
      两处口径一致（都只剩「子集不变量由共享表机器钉住」这一套口径，「不另写正则」的
      旧声称已删），且有测试护住（两侧表测试 + 反向验证）
- [x] 八类 failure kind 各有一条断言（API 层 8/8，见上表——本票无豁免项）

## 来源

- 架构评审（`improve-codebase-architecture`，2026-09-22）候选 4 拷问 Q2：
  「同病并入 / 只记录不动 / 明确排除」→ 用户裁 **只记录不动，另立一票**（顺带记两个
  未钉住的 kind）
- 决策 250（同批落档）第三段「同病另立一票、本批不动」点名本票
- 决策 194 裁决⑦（八类失败互不混淆）、决策 187（判定只有一处的原话）、
  决策 246（跨语言共享 fixture 的先例）

## 实施记录（2026-09-23）

- `is_digest_shaped` 措辞清单补实测原话 `packfile trailer mismatch`（libgit2 1.9.7、
  class=Indexer）——本票取证的**副产物**，修的是一处真分类缺口，不只是补测试；
- testkit `SmartHttp` 新增 `RemoteBehaviour`（Normal / AuthRequired / CorruptPack）与
  `serve_behaviour`，既有调用方走 `serve` 不受影响；
- `docs/glossary.md` 市场词条 `repo_unreadable` 状态由「401·404」订正为 **404**
  （`map_market_error` 实为 `not_found`→404），并**显式标注与决策 194 裁决⑦ 原文的
  不同**（按 `docs/agents/domain.md`：与决策冲突必须显式指出并引用编号）——
  同一种病（文档声称 ≠ 代码事实），顺手在此票口径内修掉；
- **又一处同病、只记录**：`PUT /market/repos` 收畸形仓名时，`RepoId::parse` 的
  `Error::Market{kind: repo_not_allowed}` 被 handler 包成「配置错误」重新映射，
  `kind` 丢失（400 报文只有 `error` 没有 `kind`）——`failure_classes` ⑥ 的注释
  「也是 400 / repo_not_allowed」只对了一半，已按实测改写注释并断言
  `kind` 为空（防两边口径再分叉）；要不要让这条路径带上 kind 属生产改动，**本票不做**；
- `docs/testing.md`：194 票 02 行的「两处覆盖损失」改记为已补 + 缺口指向，新增 250 行。
