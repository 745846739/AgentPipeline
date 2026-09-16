# 01: GitHub 仓访问层与测试接缝

**What to build:** 新增一层「从一个 GitHub 仓取技能」的访问层，作为本 effort **唯一新增的测试接缝**。
它只做三件事：探分支 tip（**不下载**）、列出仓里所有技能目录、把一个技能目录读成一个
[`SkillPackage`](../../../crates/core/src/agent/skill_import.rs)。**不碰落盘、不碰界面、不碰安装**——
那三样分别是票 02 与票 03 的事，而落盘侧的校验、同名冲突、路径穿越全部复用票 09 / 票 10 的既有入口。

**Blocked by:** None (可立即开始)

**Status:** ready-for-agent

**背景与判据：** 三处口径的裁定与实测底稿见 [`../background.md`](../background.md)（差异表在第 0 节）。
本票实现的是决策 194 的「只走 git 通道 + 钉 commit SHA」两条，`owner/repo` 白名单与失败分类**不在本票**。

## 接缝形状（决策 143 的第五条接缝换形状）

票 10 的 `MarketClient`（`crates/core/src/agent/market.rs:115`）是「索引 → 下载字节」的形状，
GitHub 模式下**没有对应物**：没有索引、没有 `sha256`、字节来自对象库。故新接缝按"仓访问"切：

```rust
// crates/core/src/agent/repo.rs（新文件）
pub trait SkillRepo: Send + Sync + 'static {
    /// 分支 tip（只 ls-remote，不下载 pack）。
    fn head(&self, repo: &RepoId) -> BoxFuture<'static, Result<Oid>>;
    /// 该 commit 下所有技能目录（含 SKILL.md 的目录即技能）。
    fn list_skills(&self, repo: &RepoId, commit: Oid) -> BoxFuture<'static, Result<Vec<SkillRef>>>;
    /// 读一个技能目录（含子树）成一个 SkillPackage。
    fn read_skill(&self, repo: &RepoId, commit: Oid, dir: &str) -> BoxFuture<'static, Result<SkillPackage>>;
}
```

`RepoId` 是**校验过的** `{owner, name}`（见下），不是字符串；`SkillRef` 至少含 `{dir, name}`，
`name` = `dir` 的 basename。生产实现 `Libgit2Repo`；离线实现在 testkit。

## 实现要点（每条都有实测背书，别当风格偏好）

1. **URL 只能由我们构造，形态只有一种**：`https://github.com/{owner}/{repo}.git`。
   理由不是洁癖——libgit2 的传输注册表里 `git://` / `http://` / `https://` / `file://` / `ssh://`
   （还有 `ssh+git://` / `git+ssh://`）全在，而且**裸文件系统路径也会被 local transport 吃掉**
   （`transport_find_fn` 判的是 `git_fs_path_exists(url) && is_dir(url)`）。用户在「添加一个仓」里
   填的那个字符串若直接当 URL 用，走哪条 transport 就由它决定。故 `RepoId` 解析必须拒绝：
   带 scheme、含 `@`、含 `..`、含多余 `/` 或空段、非 ASCII、空 owner/repo。
2. **`commit` 必须是完整 40 位十六进制。** 实测（变体 I）：7 位缩写 SHA 会让 `fetch` **返回 `Ok`
   但什么都不取**——0.74 s、"成功"、无 ref、无对象、无 `shallow` 文件、**无任何错误**。
   不校验就会报"装好了"而其实没装。
3. **`follow_redirects(RemoteRedirect::None)` 必须显式设**——`FetchOptions::new()` 的默认是
   `Initial`（跟初始请求的跨站重定向），靠默认值会当场破掉决策 177②。写代码时要知道 `None` 的
   **真实语义是"不跟跨站重定向"**：libgit2 对**同站 http→https 升级**仍然放行（`src/util/net.c`
   里只在目标 scheme 不是 https 时才拒绝跨 scheme 跳转，host 检查则被 `allow_offsite` 关掉）。
   我们只走 https，这条残余不可达——**但要写在注释里**，免得后人以为 `None` 密不透风。
   实测这条策略确实在拦：`https://www.github.com/...`（真 301 到 github.com）在 `None` 下失败，
   报 `cannot redirect from 'www.github.com' to 'github.com'`；而 `Initial` / `All` 下成功。
4. **取法固定 `depth(1)`**：实测 1.9–2.3 s，`.git` 212 KiB，`.git/shallow` 落盘且内容就是被取的
   tip；树与 blob 完整，只有历史被切（被取的 commit 报 `parents=0`，是 shallow 的 graft 效应）。
   注意 `Remote::fetch` **不会创建或移动 `HEAD`**（`HEAD` 保持 unborn），只写 `FETCH_HEAD`——
   别指望 `HEAD` 能告诉你拿到了什么，用 `update_tips` 回调或 refspec 的目标 ref。
5. **字节上限在流式回调里守**：`RemoteCallbacks::transfer_progress` 里读
   `Progress::received_bytes()`（libgit2 的注释就是 "Size of the packfile received up to now"）
   累加，超 64 MiB 就 `return false` 中断——实测回调返回 `false` 会中止并报
   `indexer progress callback returned -1`。**上限与本地导入端点的 `DefaultBodyLimit` 同值**，
   保持两条路对内存的消耗同量级。
6. **不落工作区**：`find_commit` → `tree()` → `tree.get_path(dir)` → `find_blob`。
   目录要**递归**——实测 pulumi 的技能目录里 `agents/` 是子树（`agents/openai.yaml`），
   非递归会漏掉兄弟文件，而票 07 的展开依赖它们真的落盘。
7. **缓存按 (仓, commit)**：同一 (仓, commit) 的"列技能"与"读技能"共用同一份已 fetch 的仓目录
   （票 02 的安装与票 03 的列表据此共用）。这同时是票 03「列表钉住浏览时那个 commit」能成立的前提。
8. **git2 用起来有两处坑**（实测踩到）：`Remote<'repo>` 借用了 `Repository`，取完**必须 drop**
   才能返回 `Repository`（否则 `E0505`）；`Version::libgit2_version()` 返回 `(u32,u32,u32)` 元组，
   **没有** `.major()` / `.minor()` / `.patch()` 这三个方法。
9. **本仓的 libgit2 是系统库**（`libgit2-sys` 的 build script 优先用 pkg-config 找到的
   `/usr/local/Cellar/libgit2/1.9.7`，`Version::vendored() == false`），且 **1.9.7 完全没有协议 v2**
   （整个源码树里 `ls-refs` 零命中，永远说 v0）。这正是 GitHub 那条路能走通的原因：v0 能力里
   `allow-tip-sha1-in-want` / `allow-reachable-sha1-in-want` 都在，而 v2 的广告里没有它们。
   **一处要记进风险的**：这条路依赖 GitHub 继续供 v0 与这两个能力位。

## 技能识别口径（扫描层）

**含 `SKILL.md` 的目录就是技能，名字 = 该目录的 basename，与深度无关。** 7 个流行仓 290/290 个技能
都满足这一条（深度 2–5 段，仓内每个仓各自统一），主流 CLI 的 `getSkillFolderPath()` 用的是同一个判据。

两条实现注意（都是实测踩出来的）：

- walk 时按 **basename 精确等于 `SKILL.md`** 匹配。用 `endsWith('skill.md')` / 大小写不敏感收尾匹配，
  会把 `mattpocock/skills` 的 `.changeset/add-implement-spec-skill.md` 误收（38 vs 37 的差就是它）。
- 上游技能的 frontmatter `name` 若与目录名不符，落盘前的 `validate()` 会拒（名字是唯一身份，决策 172）。
  这是**既有的正确行为**，不要在本层"顺手修好"——报错要说清是哪个技能的 frontmatter 与目录名不一致。

## 怎么把读到的东西变成 SkillPackage：走 zip，不走新构造器

`SkillPackage::from_entries_with_name` 是私有的，而 `install` 的入口是 `from_zip` / `from_dir`。
两条路可选：新增一个 `pub(crate)` 的"条目表 → SkillPackage"构造器，或者**在内存里打一个
`{name}/SKILL.md` 单根 zip 再交给既有的 `from_zip`**。

**取后者。** 理由：验收里"引擎零改动"是字面意思，而 zip 往返让**既有的每一道门都留在路径上**
（`sanitize_rel_path`、`enclosed_name` 那道独立复检、`MAX_ENTRIES` / `MAX_ENTRY_BYTES`、
`validate_single_root`、frontmatter 校验）；新增构造器则会绕过其中几道，等于为远程来源开了一条
比本地上传更短的路——而"远程包不比本地上传的包享有更宽的路"是本 system 明写的一条口径。
技能是 markdown，几十 KB 的内存 zip 往返不构成成本。`zip` crate 已在依赖里（现在只用于读），
`ZipWriter` 可直接写 store-only 条目。

## 离线 fixture（两层，各司其职）

| 层 | 用什么 | 覆盖什么 | 不能覆盖什么 |
|---|---|---|---|
| 快单测 | 本地裸仓（`git clone --bare`） | 扫目录（多形态深度）、递归读子树、三类 not_found、`RepoId` 校验 | **不能带 `depth`**——实测 local transport 直接报 `shallow fetch is not supported by the local transport`（git2 源码里也留着 FIXME「libgit2 doesn't support local shallow clones」），也覆盖不到传输策略 |
| 核心用例 | **离线 smart HTTP** | 真 HTTP 传输 + `depth(1)` + `RemoteRedirect::None` + 中断 | — |

离线 smart HTTP 的做法已经验证过：把 `git upload-pack --stateless-rpc [--advertise-refs]` 用几十行
包成一个 HTTP 服务即可，libgit2 能 fetch、能取到 commit 与 blob。**dumb HTTP 不行**：libgit2 硬校验
响应的 `Content-Type` 必须是 `application/x-git-upload-pack-advertisement`，而
`git update-server-info` + `python -m http.server` 返回 `application/octet-stream`，实测被拒
（`invalid content-type`）。fixture 里若要能取**非 HEAD 的旧 commit**，需在裸仓里开
`uploadpack.allowAnySHA1InWant true`（本机 git 默认 false）。

**这里有一个覆盖损失要记账，不能装作没少**：离线 fixture 全在本机，所以**仓/commit 不在白名单**
那类判定打不到 E2E——所以传输与来源判定必须照 `crates/core/src/agent/egress.rs:505` 的
`check_allow_host` 那样**纯函数化**，配自己的单测。

## 验收

- [ ] `cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全过
- [ ] 离线用例覆盖：ls-remote 取 tip（且断言**没有下载 pack**）；列技能（含 2–3 段与 4–5 段两种深度形态）；
      读一个技能目录（含子树兄弟文件）；三类失败 `repo_not_found` / `commit_not_found` / `skill_not_found` 分得开
- [ ] shallow 路径有离线用例（smart HTTP），断言 `.git/shallow` 存在、且被取的 commit 树可读
- [ ] 按**旧 commit** 的用例有（fixture 开 `allowAnySHA1InWant`），断言取到的是那个 commit 而不是 tip
- [ ] `RepoId` / 40 位 hex 的纯函数单测覆盖各种非法输入（scheme、`@`、`..`、缩写 SHA、空段）
- [ ] 字节上限的用例：小上限下中断，报错含"已收到 / 上限"
- [ ] 默认门**不打真网络**；打真 GitHub 的用例 opt-in（`test.skip` + 环境变量，照
      `frontend/e2e/screenshots.spec.ts` 的先例），且显式开关下能跑通
- [ ] 引擎零改动可核对：`crates/core/src/agent/skill_import.rs` 与 `market.rs` 的 diff 为空

**Notes（给实现者）:**
- 出网这一层的既有姿态可对照 `crates/core/src/agent/egress.rs`：默认不放行任何外网目标，
  且"识别不出来就归到不可判定、按拒绝处理"。本票要做的是同一姿态的另一面——**来源由白名单决定，
  而 URL 由我们拼**，两者都不要交给用户输入。
- `crates/core/src/agent/market.rs:395` 的 `origin_of` 是现成的 origin 解析，若需要"复检实际来源"
  可参考它的写法；但 GitHub 模式下 origin 恒等于 `github.com`，**它不再承担判定作用**（见决策 194）。
- 真 GitHub 的连通性会偶发中断（历史上 75 s 超时一次、raw 取较大文件超时两次），
  这也是那些用例必须 opt-in 的原因之一。
