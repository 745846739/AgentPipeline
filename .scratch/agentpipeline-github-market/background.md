# 实测底稿：GitHub 作为技能来源

> **这不是票，是本 effort 的实测底稿。** 三处口径已于 2026-09-16 经拷问 Q1–Q18 裁定并落成**决策 194**，
> 实施在 `issues/01`–`04` 四张票上。本文件的正文价值是**数据与复现命令**（文末 ①–⑦ 那几节），
> 「技能识别口径」一节仍是实施依据；而**票面性的两节（"不论选哪个都要做的前置工作"里的勾选项、
> "验收"一节）已被四张票吸收**，不再单独维护。
>
> 更要紧的是：**本文的推荐有几处已被后续事实推翻，读推荐之前先看下面这张差异表。**

## 0. 本文推荐 vs 最终裁定

| 议题 | 本文当时的推荐 | 最终裁定 | 推翻它的是什么 |
|---|---|---|---|
| 与现有 `/index.json` 那套的关系 | **可选**来源，与之并存 | **取代**：整层删净（决策 194） | 并存会把"放行判定"与"失败分类"各变成两份实现，而放行判定是本系统唯一的安全控制，不许有两份 |
| 索引从哪来（口径 1） | A′ 目录扫描 + A（`marketplace.json`）便捷层 + B 精确定位 | 只做**目录扫描**；A 留后续 | `marketplace.json` 是**插件**粒度（`wshobson/agents` 94 插件 / 183 技能），要下钻再与扫描结果合并去重，而"合并时以谁为准"无规范可依；它换来的只是分组，而分组用技能目录的父路径免费就能拿到 |
| 下载通道 | **codeload 直连**（整仓 zip） | **git 通道**（libgit2） | ① codeload 的 `content-length` 不可靠（冷缓存没有、`HEAD` 永远没有），"声明式早退"那半条在它上面是死代码；② 要"钉一个旧 commit"，zip 通道给不出可本地校验的锚；③ git 通道还免掉第二、第三个 origin |
| 摘要锚（口径 2） | A：钉 commit SHA | **A**（未变） | — |
| 镜像（口径 3） | B：上游优先 + 显式可配回退 | 第一版**只直连**，镜像另立票 | 直连实测本来是通的（`ls-remote` 1.2 s、`clone --depth 1` 2.3 s），偶发中断是抖动不是封锁；而加镜像要连同新增一个信任点、一条白名单判定面与"现在走哪个镜像"的显示 |
| 技能识别口径落在哪 | 「我们的 `skill_md_key()` 比生态窄，**这是要动的地方**」 | **引擎不动**，深度无关的判据落在新增的扫描层 | 整仓 zip 一定过不去（多技能 + `validate_single_root`），所以来源侧无论如何要选出目标目录、重打包成 `{name}/SKILL.md`；而重打包后的形态，现有 `skill_md_key()`（根级或单层）**原样接受**。`skill_import.rs:244` / `:299` 那两层是**解包层**，与**扫描层**不是一回事 |
| 信任单元 | 未提（本文只说要让镜像域名过 `allowed_sources`） | **`owner/repo` 仓级白名单** | 判定按 origin，而 GitHub 模式的 origin 恒为 `github.com`：放行一次 = 放行全世界任何作者的任何仓，正是决策 187 要避免的"配宽" |

**来源：** 2026-09-16 的实测——① 生态 registry 合规性普查（71 个主机 × 三条索引路径）；
② 摘要语义实测（GitHub ETag / 字节 vs 内容身份 / Pulumi 的 digest 漂移）；③ 镜像源实测（可达性、
重定向、字节一致性、git 通道）；④ 7 个流行仓的 archive 体积、布局与 trees API（290 个技能）；
⑤ libgit2 1.9.7 的运行时能力（shallow、按裸 SHA 取旧 commit、无工作区读 blob、重定向策略、字节上限）。
复现命令见文末；④⑤ 两组的数据在四张票里被逐条引用。

## 为什么是 GitHub

同批验证里其它来源都不通，各有各的堵法：

| 来源 | 堵在哪 |
|---|---|
| skillhub.tencent.com | 无 `index.json`；**任何地方都不给包的 sha256**；下载走明文 http + 302 跨源；包是根级 `SKILL.md` |
| skills.sh | `/index.json` 404；真能力在私定 `/api/search` + `/api/download`（`robots.txt` 还 `Disallow: /api/`）；下载是内联文件内容的 JSON，不是 zip |
| `.well-known/agent-skills/index.json`（事实约定 v2） | 71 家里**只有 6 家发布**，且**每家只发自己那 1 个技能**，无聚合型目录 |
| **GitHub** | `codeload.github.com/{o}/{r}/zip/{ref}` → **200 直返不重定向**、https → **决策 177②③ 一条都不用动** |

且 GitHub 是生态事实上的分发层：主流 CLI 的主通道就是 `skills add owner/repo`。

---

## 口径 1：索引从哪来

| 选项 | 实测 | 代价 |
|---|---|---|
| **A. 读仓内 `.claude-plugin/marketplace.json`** | **7 个流行技能仓里 6 个有**：`obra/superpowers`(★287k, 1 插件) / `mattpocock/skills`(★263k, 1) / `anthropics/skills`(★177k, 5) / `anthropics/claude-code`(★145k, 13) / `wshobson/agents`(★40k, **94**) / `pulumi/agent-skills`(★68, 4)。**缺的那个是 `vercel-labs/agent-skills`**（★31k，只有 `skills/*/SKILL.md`，无索引文件）。条目形如 `{"name":"pulumi-migration","source":"./migration",…}`——`source` 是**仓内相对目录**。`anthropics/claude-code` 那份还带 `$schema: https://json.schemastore.org/claude-code-marketplace.json`。**6 份里 `metadata.pluginRoot` 全为空**，即插件目录都相对仓根；`wshobson/agents` 与 `obra/superpowers` 另有第二处 `.agents/plugins/marketplace.json` | 它是**插件**索引不是技能索引：一个插件下挂 N 个技能，要再下钻到 `{plugin}/skills/*/SKILL.md`（实测 `wshobson/agents` 是 94 个插件 / 183 个技能）。且 marketplace 格式本身不承诺含摘要（完整性全靠 git commit）；且**它不保证存在**——缺的时候只能退回 A′ 目录扫描 |
| **B. 来源里直接写子路径**（`owner/repo@<sha>:<subpath>`） | 与 C 相比不花 API 配额；一次 zip 拉取即得。**精确定位的唯一形态**（要"就装这一个技能、并按 commit 钉死"，只能这么写） | 没有"浏览"能力——用户得自己知道要装哪个子路径 |
| C. GitHub Contents API 列目录 | **不建议**：未认证 REST 实测 `core: limit=60`/小时，`search: limit=10`。当索引用，几次浏览就耗尽——`wshobson/agents` 那种 183 个技能的仓，递归树一次就要一大截配额 | 触发 403 且报错难解释（`api.github.com` 对无 UA/频控的响应还可能是 403 而非 429） |

**推荐：A′ 目录扫描作底座 + A 作便捷层 + B 作精确定位层**（不是"A 可选"）。

`marketplace.json` 的采用率（**6/7** 流行仓）比生态那个事实约定索引（`/.well-known/agent-skills/index.json`，71 个主机里只有 6 家发布）高一个数量级，而且它就在仓里、随 commit 一起不可变——这是"GitHub 作为市场来源"最现成的入口。但**别把它当索引的唯一真相**：它只给到插件粒度，且 `vercel-labs/agent-skills` 就没有，所以底座必须是**目录扫描**（含 `SKILL.md` 的目录即技能，见文末"技能识别口径"），`marketplace.json` 只是帮助分层展示的便捷层。

## 口径 2：摘要锚在哪（**本票最要紧的一处**）

我们现有模型是**字节身份**：索引里钉住下载字节的 sha256，客户端现算比对（决策 177）。GitHub 给的是
**内容身份**——这两者对不上。三条实测：

1. **ETag 不是所收字节的 sha256。** 它看起来像（64 位十六进制），实测不是：`refs/heads/main` 的 zip
   实际 `sha256 = 937f5799…`，而 ETag = `274a83ac…`。
2. **同一个 commit，换个 URL 形态，字节就变了。** 实测：分支名取 → 237506 B / `937f5799…`；
   用同一个 commit 的 SHA 取 → 248450 B / `e19fa620…`。**而两者的 ETag 完全相同**（`274a83ac…`）。
   → GitHub 认的是内容，字节随生成方式变（顶层目录名 `{repo}-main` vs `{repo}-<sha>` + 压缩差异）。
3. **外部佐证：不锚在不可变标识上的 digest 一定会漂。** `www.pulumi.com` 的 v2 索引 15 条**全部不合格**
   ——11 条 digest 与产物不符、4 条取不到；原因是它的 `url` 指向
   `raw.githubusercontent.com/pulumi/agent-skills/main/…/SKILL.md`（`main` 是移动靶），digest 停在旧版本。
   反过来，`agentskills.io` / `docs.firebender.com` / `docs.openhands.dev` / `docs.workshop.ai` / `mux.coder.com`
   这 5 家的 5 条索引，`$schema` 正确、字段合规、产物现算 sha256 与 digest **逐位相符**。

| 选项 | 机制 | 代价 |
|---|---|---|
| **A. 钉 commit SHA**（推荐） | `owner/repo@<sha>:<subpath>` 不可变；git 的对象哈希由 libgit2 在 fetch 时**本地校验** | 需要 git 通道（`git2` 已是 core 依赖，决策 12，不引第二套）或 codeload 按 SHA 取；私有仓要凭据 → 多一份落盘密钥（决策 112 记着 provider 密钥目前是明文存的），是新的安全决定 |
| B. 钉 zip 字节 sha256 + 首装 TOFU | 复用现有一切 | **随 ref 变动即失效**——正是 Pulumi 那个失败模式；等于把"没被改过"的保证降级成"上次是它" |
| C. 钉 zip 字节 sha256 + commit-pinned URL | 把 URL 钉到 SHA 后字节在实测中稳定 | GitHub **不承诺** archive 字节可复现；实测 2 里"同一 commit 两种字节"本身就是反证。把它当地基等于赌别人的生成实现不变 |

**推荐：A。** 它与本系统的模型其实**更契合**：commit SHA 就是内容摘要，"没被改过"由 git 自己证，
比 sha256 多一层（还管目录结构），且不需要任何索引来托管摘要。

**若采纳 B 或 C**：必须同时登记一条决策，并在决策行显式标注「与决策 177 的摘要口径关系」——
177 的 `digest_mismatch` 报错文案（"期望 X，实际 Y，请与来源方核对"）在一个天然会漂的锚点上会
变成噪声。

## 口径 3：镜像怎么用

### 实测：可用且字节一致的（本次采样）

| 镜像 | raw 单文件 | 仓 zip | 重定向 |
|---|---|---|---|
| **`gh-proxy.com`** | ✓ 15734 B 逐位一致 | ✓ 237506 B / 248450 B **逐位一致** | 无（200 直返） |
| `ghfast.top` | ✓ 逐位一致 | ✗ 403（挡 zip） | 无 |
| `ghproxy.net` | ✓ 逐位一致 | ✗ 403（挡 zip） | 无 |
| `cdn.jsdelivr.net` / `gcore.jsdelivr.net` | ✓ 逐位一致（按 `@main` / `@<sha>` 寻址） | — 不提供 zip | 无 |

失效的（省得再踩）：`ghproxy.cc`、`gh.llkk.cc`、`hub.gitmirror.com`、`raw.gitmirror.com`、
`raw.fastgit.org`、`cdn.statically.io`、`bgithub.xyz`、`kkgithub.com` 均不可达；`gitclone.com` 回 404。

### 实测：直连本来就是通的

早先一次 `git clone` 失败（连 `github.com:443` 超时 75 s）是**瞬时抖动**，不是结构性封锁。复测：
`git ls-remote` 直连 **1.2 s**、`git clone --depth 1` 直连 **1.8 s**、经镜像 **2.0 s**，全部成功；
两种取法的 commit（`9b794aec…`）、整棵树（`38f725b0…`）、技能子目录（`5cf629b0…`）哈希**完全一致**。
大传输仍会偶发中断（当初就断在 packfile 阶段；raw 取较大文件也超时过两次）。

### 选项

| 选项 | 说明 |
|---|---|
| A. 只直连 | 少一个信任点；但偶发中断时用户无路可走 |
| **B. 上游优先 + 显式可配的镜像回退**（推荐） | 例如 `[market] github_mirror = ""`（空 = 直连，非空才走前缀）。上游通就用上游，不通用户自己填 |
| C. 固定走镜像 | 不推荐：多一个默认信任点，且镜像域名必须进 `[market] allowed_sources`——**等于把一个不可控的第三方域名写进"允许下载引导 agent 正文"的名单**（决策 187 的原话），这个决定该由用户显式做，不该是默认值 |

### 镜像改变了什么（这条要写进实现结论）

`gh-proxy.com` 对 zip 是**纯透传**（字节与上游逐位一致）。于是口径 2 的"字节 vs 内容身份"错配在镜像
路径上被意外绕开了——钉上游字节的 sha256 仍然成立。**但这是它的实现方式，不是它承诺的契约**；本次
采样只是一个仓、两个 ref 形态各一次。所以：

- **锚在 commit SHA（口径 2 的 A）时**：镜像只是加速器，伪造不了内容（git 本地校验）。
- **锚在 zip 字节 sha256 时**：安全性建立在"镜像不重新压缩"这个第三方实现细节上——能跑，地基是别人的代码。

---

## 不论选哪个都要做的前置工作

- [ ] **必须重打包——这不是 Pulumi 的特例，是全部流行仓的常态。** 实测 7 个流行技能仓、共 **290 个技能**，
      `SKILL.md` 在仓内的段数**没有一个是 1**（1 = 仓根），全是 2–4；叠上 GitHub zip 自动加的
      `{repo}-{ref}/` 前缀后就是 3–5。而 `skill_md_key()`（`crates/core/src/agent/skill_import.rs`）
      只认**仓根级**或**单层** `*/SKILL.md`，所以**这 290 个技能一个都过不去**（Pulumi 只是其中"段数 4"
      的那一种形态，不是异类）：

      | 仓 | ★ | 技能数 | 技能目录的父路径（仓内） |
      |---|---|---|---|
      | `obra/superpowers` | 287k | 14 | `skills/` |
      | `mattpocock/skills` | 263k | 37 | `skills/{分组}/`（engineering 18 / in-progress 8 / productivity 7 / misc 4） |
      | `anthropics/skills` | 177k | 20 | `skills/` ×19 + **仓根 ×1**（那一个是 `template/SKILL.md`） |
      | `anthropics/claude-code` | 145k | 10 | `plugins/{插件}/skills/` |
      | `wshobson/agents` | 40k | 183 | `plugins/{插件}/skills/`（94 个插件） |
      | `vercel-labs/agent-skills` | 31k | 9 | `skills/` |
      | `pulumi/agent-skills` | 68 | 17 | `{分组}/skills/` ← **这一个是少数派写法** |

      流程必须是：解 zip → 找到该技能目录 → 重打成 `{name}/SKILL.md` → 交给既有 `install`。
- [ ] **技能识别口径要按生态的来：`SKILL.md` 的父目录就是技能目录，名字就是该目录的 basename，与深度无关。**
      上表 7 种形态全部满足这一条（290/290）。主流 CLI 用的就是这个判据——`skills` 的
      `getSkillFolderPath(path)` 实现的正是"砍掉结尾的 `/skill.md`"。
      **我们现有的 `skill_md_key()`（只认根级或单层）比生态口径更窄**，这是要动的地方；若沿用旧口径，
      每接一个新来源都要为它的目录习惯再打一次补丁。
      连带一条实现注意：walk 时要按 **basename 精确等于 `SKILL.md`** 匹配，别用 `endsWith('skill.md')`
      ——实测 `mattpocock/skills` 的 `.changeset/add-implement-spec-skill.md` 会被误收（38 → 37 的差就是它）。
- [ ] **下载形态必须用 codeload 直连。** `https://codeload.github.com/{o}/{r}/zip/{ref}` 是 200 直返；
      而 `https://api.github.com/repos/{o}/{r}/tarball/{ref}` 是 **302** 跳 codeload——走这条会被
      决策 177② 的 `Policy::none()` 当场拒掉。
- [ ] **网络测试必须 opt-in**（`test.skip` + 环境变量，照 `frontend/e2e/screenshots.spec.ts` 先例）。
      本机对 GitHub 的连通性会偶发中断，放进默认门就是给自己埋 flaky。
- [ ] **E2E ⑬ 的离线 fixture 保留**（`frontend/e2e/marketRegistry.ts` + `market-install.spec.ts`）：
      它钉的是安装通路本身，与来源是谁无关；真 GitHub 的用例是**另加**，不是替换。
- [ ] **粒度与上限说明**：codeload 只能整仓下载（实测该仓 237–248 KB），大 monorepo 会顶到
      `MAX_DOWNLOAD_BYTES = 64 MiB`；超限要给可操作报错，不能只说"包过大"。
- [ ] 若有镜像开关：镜像 origin 与上游 origin **分别**过白名单判定（复用 `source_allowed`），
      并在界面上说清"现在走的是哪个镜像"。

## 验收

- [ ] 三处口径选定，且在实现结论里逐条对照上面的实测数据说明选了什么、为什么
- [ ] 若未采纳推荐项 → 已登记决策并在决策行标注与决策 177 的关系
- [ ] 引擎零改动（`install_from_market` / `verify_digest` / `SkillPackage::from_zip` 的 diff 为空）
- [ ] `cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全过
- [ ] 真 GitHub 的用例默认 skip，显式开关下能跑通；离线 E2E ⑬ 仍全绿

**Notes（实现提示）:**
- 走 **git 通道**（`git2`，已是 core 依赖）比走 zip 更贴合本系统：commit SHA 即摘要，不需要索引托管
  摘要，且目录结构也在哈希覆盖内。代价是依赖 git 传输（本机实测会偶发中断），私有仓多一份凭据。
- 走 **zip 通道**（codeload）更接近既有 `HttpMarketClient` 的形状（一个 URL → 字节），复用面最大，
  但必须自己扛"字节身份"这件事（见口径 2）。
- 两者可以并存：git 通道给"按 commit 精确安装"，zip 通道给"给个 URL 就能装"。

---

## 复现命令（本轮所有数字都是这些命令跑出来的）

```bash
# ① 生态合规性普查：三条索引路径 × 一批主机
#    判据：content-type 是 JSON **且**正文能解析成 JSON。返回 200 + text/html 的一律算「未发布」
#    ——SPA / 404 页伪装成 200 是这轮最常见的假阳性（逐条验过正文，都是 <!DOCTYPE html>）。
for host in www.skills.sh skillhub.cn clawhub.ai agentskills.io smithery.ai; do
  for p in /.well-known/agent-skills/index.json /.well-known/skills/index.json /index.json; do
    printf '%-28s %-42s ' "$host" "$p"
    curl -sS -m 10 -o /dev/null -w '%{http_code} %{content_type}\n' "https://$host$p"
  done
done

# ② GitHub 直连是否重定向（不带 -L）
curl -sS -o /dev/null -D - https://codeload.github.com/pulumi/agent-skills/zip/refs/heads/main | head -3
curl -sS -o /dev/null -D - https://api.github.com/repos/pulumi/agent-skills/tarball/main | grep -i location

# ③ 字节 vs 内容身份：同一 commit 两种 URL 形态，字节不同但 etag 相同
curl -sS https://codeload.github.com/pulumi/agent-skills/zip/refs/heads/main -o /tmp/a.zip
curl -sS https://codeload.github.com/pulumi/agent-skills/zip/9b794aec9c4169f137285c2763c06064d247dd47 -o /tmp/b.zip
shasum -a 256 /tmp/a.zip /tmp/b.zip   # 937f5799… / e19fa620…

# ④ Pulumi 的 digest 漂移（外部佐证）
curl -sS https://www.pulumi.com/.well-known/agent-skills/index.json | head -20
curl -sS https://raw.githubusercontent.com/pulumi/agent-skills/main/pulumi/skills/pulumi-best-practices/SKILL.md | shasum -a 256

# ⑤ 镜像：可达性 / 是否重定向 / 字节是否一致
curl -sS -o /dev/null -D - https://gh-proxy.com/https://codeload.github.com/pulumi/agent-skills/zip/refs/heads/main | head -3
curl -sS https://gh-proxy.com/https://codeload.github.com/pulumi/agent-skills/zip/refs/heads/main | shasum -a 256  # 与 /tmp/a.zip 一致

# ⑥ git 通道（含镜像）
git ls-remote --heads https://github.com/pulumi/agent-skills.git main
git clone --depth 1 https://gh-proxy.com/https://github.com/pulumi/agent-skills.git /tmp/cl

# ⑦ 流行仓的 SKILL.md 布局（数段数）+ marketplace.json 是否在
#    每仓 2 次 API 调用（repos 取默认分支 + git/trees 递归）；未认证配额 60/小时，探 7 个仓够用
for r in obra/superpowers mattpocock/skills anthropics/skills anthropics/claude-code \
         vercel-labs/agent-skills wshobson/agents pulumi/agent-skills; do
  br=$(curl -sS "https://api.github.com/repos/$r" | python3 -c 'import json,sys;print(json.load(sys.stdin)["default_branch"])')
  printf '%-26s ' "$r"
  curl -sS "https://api.github.com/repos/$r/git/trees/$br?recursive=1" | python3 -c '
import json,sys
from collections import Counter
t=json.load(sys.stdin)
# 注意是按 basename 精确匹配：endsWith("skill.md") 会把 .changeset/xxx-skill.md 误收
ps=[e["path"] for e in t["tree"] if e["type"]=="blob" and e["path"].split("/")[-1].lower()=="skill.md"]
print(len(ps),"个技能; 段数分布", dict(sorted(Counter(len(p.split("/")) for p in ps).items())))'
done

# ⑦b marketplace.json 是否在（6/7 在；vercel-labs/agent-skills 是 404 那个）
#     raw.githubusercontent.com 在本机会偶发超时，故带 --retry（实测 7 次里 4 次超时，
#     超时的几个用 API 的 contents 接口复核过，结论一致）
for r in obra/superpowers mattpocock/skills anthropics/skills anthropics/claude-code \
         vercel-labs/agent-skills wshobson/agents pulumi/agent-skills; do
  printf '%-26s ' "$r"
  curl -sS --retry 3 --retry-all-errors -m 20 -o /dev/null -w '%{http_code}\n' \
    "https://raw.githubusercontent.com/$r/refs/heads/main/.claude-plugin/marketplace.json"
done
```

## 技能识别口径（三个来源交叉印证后的结论）

| 口径 | 判据 | 来源 |
|---|---|---|
| 生态约定 v2 | 索引条目给 `name` + `type` + `url` + `digest`，产物是单个 markdown 或 archive | `/.well-known/agent-skills/index.json` 的 `$schema` |
| 主流 CLI | 找到 `SKILL.md` → **砍掉结尾的 `/skill.md` 就是技能目录**，与深度无关 | `skills` 包 `dist/cli.mjs` 的 `getSkillFolderPath()` |
| 7 个流行仓实测 | **290/290** 个技能都满足「技能名 = `SKILL.md` 父目录的 basename」 | 本文件 ⑦ 那条命令 |

三条指向同一件事：**技能是"含 `SKILL.md` 的目录"，不是"某个固定深度的路径"。** 我们的
`skill_md_key()`（根级或单层）是目前唯一更窄的实现，接 GitHub 时它是要改的那一处。

> 顺带一条边界：`mattpocock/skills` 里那些技能名（`ask-matt` / `code-review` / `codebase-design` /
> `diagnosing-bugs` / `domain-modeling` …）与本机 `~/.zcode/skills/` 下的同名技能是同一批。
> 也就是说这条路一旦通了，"从上游按 commit 装一个指定版本的技能"就能替代现在的手工拷贝。
