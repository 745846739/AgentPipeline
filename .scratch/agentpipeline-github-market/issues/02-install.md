# 02: 从一个钉住的 commit 安装技能

**What to build:** 把票 01 的读取层接到**既有落盘入口**上，做到"点一下，把这一个 commit 的技能装进技能根"。
含四件事：钉住 commit、同名冲突报文带上来源、八类失败、来源记录。**落盘侧的校验、覆盖语义、路径穿越
一行不改**——远程包不比本地上传的包享有更宽的路，这是本系统明写的一条口径。

**Blocked by:** 01

**Status:** done（2026-09-17 回填：实现早已落地，随决策 194 于 d41051f 交付）

## 链路

```
界面传来 (repo, commit, dir)  ← 票 03 的列表里那一行，commit 是列表当时那个
  → SkillRepo::read_skill   ← 票 01，按 (仓, commit) 缓存，不重复拉
  → 内存 zip 往返（{name}/SKILL.md 单根）
  → SkillPackage::from_zip → validate → install   ← 既有入口，零改动
  → 写一行来源记录（本票新增的唯一持久化）
```

**`commit` 必须一路透传、不得在中途"取最新"**：用户在列表里看到的是某一份，装到的就必须是那一份。
这是"看到的 = 装到的"唯一落点，也是钉 commit 这个选择的全部意义（否则锚退化成"安装那一刻的 HEAD"，
回到 Pulumi 那个移动靶的形态）。界面负责显示"列表基于 `<短 SHA>`（时间）"，后端负责不再解析 HEAD。

## 同名冲突：报文要能说出"装的是哪个仓哪个版本"

决策 172 的裁定不变——**技能名是唯一身份，同名默认拒绝**，`overwrite` 才覆盖。变的只有报文的来源部分：
现在的"当前来源"是从路径推出来的（技能根下那份 `SKILL.md` 的路径），而 GitHub 模式下仓名、commit、
子路径一样都不在路径里。故改为读来源记录：`owner/repo@<短 SHA>:<子路径>`；
**没有记录时回落到现在那句路径**（手工拷进来、本地导入、扫描进来的技能都没有记录，不能因此报不出来源）。

界面已在用的那个串（`同名已存在，覆盖？`）不要改，它是既有 E2E 的锚点。

## 八类失败（判据不变：每类对应一个互不相同的用户动作）

| 类 | HTTP | 用户要做的动作 |
|---|---|---|
| `market_network` | 502 | 重试（下游不可达） |
| `repo_not_found` | 404 | 改仓名 |
| `commit_not_found` | 404 | 改/换 commit（那个 SHA 取不到） |
| `skill_not_found` | 404 | 换技能（这个仓里没有它） |
| `repo_unreadable` | 401 / 404 | 换仓，或知道本版**不支持私有仓** |
| `digest_mismatch` | 400 | 别装，报警（对象哈希不符，语义比票 10 的字节 sha256 更强） |
| `repo_not_allowed` | 400 | 去界面把这个仓加进白名单 |
| `download_too_large` | 400 | 换更小的仓，或改指一个子目录 |

两处要单独说：

- **`repo_unreadable` 的可判定性要先验，不许直接假定。** 本票里**第一个要做的动作**是拿一个已知
  私有仓跑一次 `ls-remote`，看 libgit2 给的是 401 系列还是干脆 404：若与"仓不存在"同形，就把这一
  类**并进 `repo_not_found` 的文案**（"也可能是无权访问；本版不支持私有仓"），而不是硬造一个判不出来
  的类。决策 194 之所以明确"不做私有仓但报错要说清"，就是为了不让用户把无权限误读成自己拼错了仓名。
- **超限必须在中断那一刻报。** 票 01 的流式字节回调会 `false` 中断 fetch，本票负责把它翻成一句可操作
  的话：**已收到多少 / 上限 64 MiB / 怎么办**（改指子目录或换更小的仓）。不能只说"包过大"——
  这条来源下的用户是**已经等了一会儿**才被拒的（codeload 的 `content-length` 不可靠，`HEAD` 永远不返回，
  所以没有"下载前先问大小"这条路；git 通道同理，只能边收边判）。

**决策 181 的失败映射要同批改**：它末尾写着"复用票 10 的四类市场失败映射（技能不存在 404 /
来源未放行 400 / 摘要不符 400 + `detail` / 网络失败 502）"，那条映射活在 `POST /skills/install` 里，
不跟着换就会变成"八类里有两类永远映射不到、界面按四类分支"。

## 来源记录（本票唯一新增的持久化）

新迁移 `0011_skill_sources.sql`，单行一技能：

```sql
CREATE TABLE IF NOT EXISTS skill_sources (
    name         TEXT PRIMARY KEY,
    owner        TEXT NOT NULL,
    repo         TEXT NOT NULL,
    commit_sha   TEXT NOT NULL,
    subpath      TEXT NOT NULL,
    installed_at TEXT NOT NULL
);
```

> 号数是 **0011**，不是 0010：票 03 的界面仓名单也要一张新表，排在前面（`0010_market_repos.sql`）。
> 两张表同批落地，各占一个迁移文件——**一张迁移一件事**，回滚与追责都干净。

**为什么不往技能目录里写元数据文件**：技能目录里的任何文件都会进票 07 的**兄弟文件展开**——
`from_zip` 专门过滤 `__MACOSX` / `.DS_Store`（`skill_import.rs:112` 的 `is_archive_junk`）正是为了
不让非技能内容混进去。我们自己的元数据文件是同一类噪声，而且比 `.DS_Store` 更坏：它会被当成"这个技能
的兄弟文件"参与展开。故记机器级事实，记在库里（与迁移 0008 / 0009 那两张机器级单行表同族）。

**卸载要一并删记录**：`DELETE /skills/{name}` 之后若记录还在，下一次同名安装的冲突报文会报一个
已经不存在的技能曾经从哪儿来。停用/启用（阶段配置里的引用）与记录无关，不联动。

## 一键安装的连带（决策 181⑤⑦）

- **推荐清单要能定位**：`skill_preview::STAGE_RECOMMENDATIONS` 从"名字 + 理由"升级为再带
  `owner/repo` 与技能目录。清单**仍是界面投递的数据**（决策 172① 的裁定不变），加字段不动它的性质；
  不加的话，"还没装的人"这条主路径就断了——清单的全部意义就是给还没装的人照着装。
- **"本地已有同名 markdown 就跳过下载"那条分支要重写**：它当初（决策 181⑦）是为了让"已装但没在这个
  阶段启用"走得通，而 GitHub 模式下"已装"还多了一个维度——**装的是哪个 commit**。
  取法：本地已装 + 来源记录里的 `(owner, repo, commit, subpath)` 与清单一致 → 跳过下载只写配置（现状）；
  **不一致 → 不跳过**，让它照常走安装从而撞上同名冲突，由用户显式选覆盖。**不要静默换成旧版，
  也不要静默升级**——两种沉默都会让"我配置里引用的是哪一份技能"变得不可知。

## 私有仓：本版不做（决策 194）

不提供凭据入口，界面上也不放 token 输入框。可行性已经量过：`FetchOptions::custom_headers` 能逐字转发
`Authorization`（实测 `Authorization: Basic …` 在 `info/refs` 与 `git-upload-pack` 两跳都到了服务端），
所以日后要做是**纯增量**、且凭据可以只从环境变量读而不落盘（不触决策 112 那条"provider 密钥目前明文存储"）。
本票只承担它的报错面（见 `repo_unreadable`）。

## 验收

- [x] `cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全过
- [x] 离线用例（票 01 的 smart HTTP fixture）覆盖：装成功、装到的是**被钉的那个 commit**（不是 tip）、
      同名冲突报文含 `owner/repo@<短 SHA>:<子路径>`、`overwrite` 覆盖成功、至少三类失败分得开、
      超限中断且报错含"已收到 / 上限"
- [x] 卸载后来源记录被清；无记录时的冲突报文回落到路径形态
- [x] 一键安装：清单带定位字段后能装；本地已装且 commit 一致时跳过下载（`note` 说明未重新下载）；
      commit 不一致时不跳过、撞冲突门
- [x] **引擎零改动可核对**：`skill_import.rs` 与 `market.rs` 的 diff 为空（本票只新增调用方与一张表）
- [x] 迁移在既有库上能升（启动即迁移），且在新空库上一样能建
- [x] 默认门不打真网络

**Notes:** 缓存与票 01 共用（同一 (仓, commit) 的列表与安装不重复拉）；`repo_unreadable` 的可判定性
结论要写进票或代码注释，别只留在会话里。

## 实施记录（2026-09-16）

**两处与票面字面不同的地方，都有理由，别当成漏做：**

1. **「已装但记录指着别处 → 不跳过」的判据放宽了一格**：`installed_is_the_listed_one` 把
   **「没有来源记录」算成一致**（本地导入 / 手工拷进来 / 扫描进来的技能都没有记录）。票面字面
   （"不一致则不跳过"）针对的是**有记录却指着别处**那种真歧义；若按字面执行，"本地先放一份、
   再用一键安装去启用"这条**离线路径**（决策 181⑦ 明确要保住的）会被打成一次网络请求，而它的
   存在理由就是"未配来源时也走得通"。跳过时**原样保留**记录里的 commit，故两种沉默都不会发生
   （既不静默换旧版，也不静默升级）。
2. **「不在推荐清单里」→ 404 + `kind = skill_not_found`**（票面未指定状态码）：见票 04 的核对结论。

**`commit_not_found` 的判据落地成了"问本地对象库"**（票 01 记了实测）：GitHub 对不存在的 `want`
回 HTTP 200 + `ERR upload-pack: not our ref`，libgit2 把那句话丢了、class 也落在 `Net`
（与真连不上同形）。故 `fetch` 返回 `Err` 之后仍 `find_commit`：找不到 ⇒ `commit_not_found`；
另配 `is_transport_shaped`（措辞清单 + `Http`/`Ssl`/`Ssh` 三个 class）当前置，
`repo_unreadable` / `repo_not_found` 仍走各自那几类。

**`digest_mismatch` 的实现与覆盖**（代码评审补的）：原先八类里只有这一类**没有任何代码路径能产生**
（常量、文案、界面提示都在，构造器不存在）。现已补上 `is_digest_shaped`（`ErrorClass::Sha1` 或
`hash mismatch` / `checksum` / `corrupt` / `invalid object`）与 `digest_mismatch` 构造器，并**前置**到
"对象不在本地就是取不到"那条判定之前——两者都会让 `find_commit` 找不到对象，顺序错了会把
"别装、报警"报成"换一个 commit"。**离线 fixture 造不出这个失败**（要手搓一个哈希坏掉的 pack），
故它由 `repo.rs` 的两条单测按 class 与措辞钉住，没有端到端用例。

**用例分布**：`crates/app/tests/market.rs` 23 条（真 libgit2 打离线 smart HTTP fixture）；
`crates/app/tests/api_contract.rs` 保留 104 条，其中阶段推荐与一键安装那 5 条改成
**「先把技能放进技能根 → 一键只写配置」**的离线形态（下载与覆盖那条链路由 market.rs 用真 fixture
覆盖，两边不重复）。

---

## 实施收尾（2026-09-16）

验收全过。闸门读数：`cargo fmt --all -- --check` 干净、`clippy --workspace --all-targets -- -D warnings`
干净、`cargo test --workspace` **全绿**（app 契约 104 + 技能来源 25 + core 448 单测等）；
前端 vitest **350**、`svelte-check` 0 错 0 警告、`vite build` 通过；Playwright 全量
**43 passed / 2 skipped**（跳过的是"截图作为证据"那两条）。真 GitHub 冒烟在显式开关下**实测一轮通过**。
