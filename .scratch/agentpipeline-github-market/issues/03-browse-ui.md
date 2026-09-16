# 03: 浏览与安装页（「设置 · 技能市场」改造）

**What to build:** 把「设置 · 技能市场」页（决策 187）从**"放行哪些来源 + 按关键词搜"**改成
**"我订阅了哪些仓 + 它们里有什么 + 装哪一个"**。页面骨架不动——那两栏版式与右列的三项预览正是
为"看着预览决定装不装"设计的；变的是左列上半：来源白名单 → **仓名单**；下半：搜索命中 → **该仓的技能列表**。

**Blocked by:** 01、02

**Status:** ready-for-agent

> 阻塞边的一处更正：此前口头说的是"C 只阻塞于 A"，但本票的验收要求**安装按钮可用**，那要等票 02 的
> 端点。所以正式阻塞边是 01 + 02。**浏览那半（仓名单、列表、预览、SHA 显示、刷新）在 01 落地后即可开工**，
> 不必干等。

## 冻结的端点契约（前后端按此并行，改一处要两边同改）

后端在 `crates/app/src/routes/market.rs` 实现，前端在 `api/client.ts` + `api/types.ts` 调用。

```
GET    /market/repos
PUT    /market/repos      body {"repos": ["owner/repo", …]}
DELETE /market/repos
  → 200 {"repos": […], "origin": "settings"|"config", "recommended": ["obra/superpowers", …]}

GET /market/skills?repo=owner/repo&q=<关键词>&refresh=1
  → 200 {"repo":"owner/repo", "commit":"<40 位>", "commit_short":"<7 位>",
         "listed_at":"<RFC3339>",
         "groups":[{"path":"skills", "skills":[
             {"name":"grill","dir":"skills/grill","description":"拷问设计树"}]}]}
  groups 按 path 升序（根级技能的 path 是空串）、组内按 name 升序。
  q 是对已 fetch 那一份的**本地过滤**（名字或描述命中，空 = 全部）。
  refresh=1 → 重新 head()，否则用缓存里那个 commit（`listed_at` 是它被取到的时刻）。

POST /market/install    body {"owner":"…","repo":"…","commit":"<40 位>",
                             "subpath":"skills/grill","overwrite":false}
  → 200 {"skill":{"name":"grill","description":…,"sibling_count":2}}
```

**错误体统一加一个机器可读的 `kind`**（`ApiError` 新增可选字段，与既有 `error` / `detail` 并列）：

```json
{ "error": "面向用户的中文提示", "detail": "原始诊断", "kind": "commit_not_found" }
```

`kind` 的取值就是票 02 那张表的八类：`market_network` / `repo_not_found` / `commit_not_found` /
`skill_not_found` / `repo_unreadable` / `digest_mismatch` / `repo_not_allowed` / `download_too_large`。
**界面按 `kind` 分支，不要按状态码、更不要按 `error` 里的字样**——那是把分类白做（`repo_not_found`
与 `commit_not_found` 都是 404，只有 `kind` 分得开，而它们要用户做的事完全不同）。

## 左列上半：仓名单

- `＋ 添加一个仓`，输入 `owner/repo`；列表里每条可删。**添加 = 放行**，它就是新的信任单元
  （决策 194：判定按 `owner/repo`，不再按 origin——GitHub 模式下 origin 恒为 `github.com`，
  按 origin 放行等于放行全世界任何作者的任何仓）。
- **两级结构照决策 22 / 56 与迁移 0009 的形状继承**：界面这份住 DB（新迁移，单行表 + `CHECK (id = 1)`），
  `config.toml` 的 `[market] github_repos` 是声明式默认；**保存即生效**（当场换，不重启），
  清掉界面这份就回到配置文件。`Some(vec![])`（显式清空 = 一个仓都不放行）与 `None`（没保存过 = 读配置）
  必须分得开——这是迁移 0009 的注释里已经写明的一条，别在新表上丢掉。
- 校验与归一只有一处实现（放行一个仓 = 允许从它下载引导 agent 的正文），照
  `config::validate_market_sources` 的姿态：配置与界面共用同一个函数，`owner/repo` 的合法性判定
  （票 01 的 `RepoId`）也是同一处，**不要在界面上另写一份正则**。

## 冷启动：内置一份只读推荐名单

Q1 选的是"含浏览发现"，但实测结论是**生态里没有聚合目录**（71 个主机 0 家发布我们的索引；事实约定
那 6 家各只发自己的 1 个技能），而仓级白名单又把浏览面收在"我加过的仓"上。于是这一页冷启动是空的，
用户得凭空知道一个仓名——一个叫"浏览发现"的页在冷启动时是空白输入框，等于把这一档的意义抹掉一半。

故内置一份**只读推荐名单**（本 effort 实测过的那些公开技能仓，例如 `obra/superpowers`、
`mattpocock/skills`、`anthropics/skills`、`vercel-labs/agent-skills`、`wshobson/agents`、
`pulumi/agent-skills`），界面上说明它只是"帮你起步"。

**关键约束：内置 ≠ 放行。** 不在名单里点"添加"之前，**一个字节都不下载**（不 fetch、不 head），
它只是若干条**用户可以删的配置默认值**，不是一份审核过的目录。这一条守住，Q6 选仓级白名单与
决策 187 的代价曲线才不被这次的便利性侵蚀。

## 左列下半：技能列表

- **按技能目录的父路径分组**，不摊平。`wshobson/agents` 是 183 个技能 / 94 个插件，`anthropics/claude-code`
  是 `plugins/{插件}/skills/`，摊平了没法看；而父路径是**扫描时免费得到的**，不需要读 `marketplace.json`
  （那正是本 effort 排除掉的那一层）。分组只影响呈现——传给票 02 的仍是 (仓, commit, 技能目录路径) 这一个身份。
- 每行显示技能名 + frontmatter 的 `description`（整仓已在本地对象库里，读 N 个 `SKILL.md` 不走网络）。
- **搜索退化成对"已 fetch 过的仓"的本地过滤**。不引 GitHub search API：票 05 的裁定是不引 API 面，
  且那个接口的配额是 10 次/小时。跨仓搜索因此只覆盖已拉下来的仓——这是有意的取舍，界面上不要假装它能搜全。
- **列表顶部显示"基于 `<短 SHA>`（时间）"+ 一个刷新动作**。刷新 = 重新 `head()`。
  这是"看到的 = 装到的"在界面上的落点：列表钉住那一刻的 commit，直到用户显式刷新。
- 未放行的仓**不进列表**（看不到装不上的东西），与放行判定同口径。

## 右列：沿用既有预览

`.prev-col` 的三项（① 推荐去向 ② 注入模式与信任态 ③ 正文特征）与安装按钮原样保留——它们是票 11 的产物，
且"装前预览"这条正是"摘要 ≠ 安全"的承担者（决策 177 明确把善意性交给它）。本票只把它的输入从
"市场命中条目"换成"票 01 读出来的技能包"，**不要顺手重排它的结构**：既有 E2E 与用户预期都锚在
`.prev-col` / `.prev-head` / `.hit` 这些串上。

失败也要在界面上**分得开**：八类失败的存在意义就是每类对应一个不同的动作（改仓名 / 改 commit / 换技能 /
去加白名单 / 换更小的仓…），界面把它们都渲染成"装不上"就等于把分类白做了。

## 明确不做

- 不新开一页"浏览"：拆页要引入"当前选中的仓"这份跨页状态，而它必然与 DB 里那份仓名单产生第二处真相；
  而决策 187 要继承的两个特性（保存即生效、清掉回配置）在一页里是同一个控件的行为，拆页后要各写一次。
- 不做跨全 GitHub 的技能搜索（见上）。
- 不放私有仓凭据入口（决策 194）。

## 验收

- [x] 离线 E2E（起票 01 的 smart HTTP fixture，走**真 libgit2 路径**）覆盖：添加一个仓 → 列表出技能并按
      父路径分组 → 右列三项预览 → 装 → 同名冲突 → 覆盖安装成功
- [x] 仓名单两级：保存即生效（不重启）；清掉界面这份回到 `config.toml`；显式清空 ≠ 未保存过
- [x] 冷启动名单：未点"添加"之前**没有任何网络请求**（fixture 的请求日志可断言）
- [x] 列表显示"基于 `<短 SHA>`"；点刷新后换成新 SHA，且装载用的仍是列表上那一份
- [x] 默认门不打真网络；`frontend/e2e/market.spec.ts`（E2E ⑫）的旧断言（"白名单是空的 / 不允许远程安装"）
      按新语义重写
- [x] `npm run check`（svelte-check）、vitest、build 全过

**Notes:** 页面上有几处既有串被 E2E 与用户预期锚着，改造时保留：占位符 `技能名或描述关键词（留空 = 列出全部）`、
`＋ 添加`、`保存`、`.blank` / `.err` / `.ok` / `.hit` / `.prev-col` / `.prev-head`、以及冲突那句
`同名已存在，覆盖？`。仓的输入框是新控件，可以按 `owner/repo` 的形态另给占位符。

---

## 实施收尾（2026-09-16）

验收全过。闸门读数：`cargo fmt --all -- --check` 干净、`clippy --workspace --all-targets -- -D warnings`
干净、`cargo test --workspace` **全绿**（app 契约 104 + 技能来源 25 + core 448 单测等）；
前端 vitest **350**、`svelte-check` 0 错 0 警告、`vite build` 通过；Playwright 全量
**43 passed / 2 skipped**（跳过的是"截图作为证据"那两条）。真 GitHub 冒烟在显式开关下**实测一轮通过**。
