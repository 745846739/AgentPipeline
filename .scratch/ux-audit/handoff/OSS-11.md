# OSS → 编排者（票 11）：三处 **不由 OSS 名下** 的发现需要指派

票 11 的检查已经跑完（逐类记录见 `.scratch/ux-audit/oss-readiness.md`）。以下三处命中
**不在 `parallel-brief.md` §二的所有权表里**，OSS 未动手，请编排者指派或自行改。
**这三处都不是「必须先改才能公开」的阻塞项**，是「公开后读者会看到本机痕迹 / 跟着一条走不到的引用」，
建议在公开动作之前顺手清掉。

---

## 1. `docs/testing.md:228` —— 引用指向被 gitignore 的产物

**目标文件**：`docs/testing.md`（所有者：**编排者**）

**原文（:228）**

```
（`.scratch/shots/app/*.png`），**默认 skip**，需 `AGENTPIPELINE_SHOTS=1` 才跑——截图是证据不是门
```

**要什么**：`.scratch/shots/**/*.png` 被 `.gitignore:24` 排除，公开仓库里这条路径不存在。
两种处置任选（都不动行为）：

- 改成不含路径的说法，例如 `（截图落在本机 `.scratch/shots/`，不入库）`，或
- 保留路径但显式标注「本机可再生证据，不随仓库公开」。

**为什么**：票 11 的验收点要求「公开视图下仓库内的链接都能走到（没有指向被排除文件的引用）」——
这是全仓仅有的两处之一（另一处见第 2 条）。

---

## 2. `design/theme-6-pixel.md:265` —— 同上

**目标文件**：`design/theme-6-pixel.md`（所有者：**DEC-VIS**，票 14 / 16 / 18）

**原文（:265）**

```
node scripts/make-icon.mjs --preview # .scratch/shots/icon-preview.png（16/32/256 在深浅两底的放大对照）
```

**要什么**：`icon-preview.png` 落在 `.scratch/shots/`，被 `.gitignore:24` 排除。
这里它是**注释里的输出路径**（不是 markdown 链接），可以：

- 把注释改成 `# 输出到本机 .scratch/shots/icon-preview.png（不入库）`，或
- 直接把这一小段注释删掉（命令本身信息量已经够）。

**为什么**：同第 1 条。注意 DEC-VIS 与 OSS 的文件所有权不重叠，故走 handoff。

---

## 3. 两处真实的 `/Users/lazyking/...` 本机绝对路径（**已入库**）

**目标文件**（两者都不在所有权表里）：

- `.scratch/shots/capture.mjs`
- `.scratch/agentpipeline-markdown-skills/issues/03-config-landing-docs-and-tests.md`

**原文**

```
.scratch/shots/capture.mjs:4:const root = '/Users/lazyking/Documents/AgentPipeline/design';
.scratch/shots/capture.mjs:5:const out = '/Users/lazyking/Documents/AgentPipeline/.scratch/shots';
.scratch/agentpipeline-markdown-skills/issues/03-config-landing-docs-and-tests.md:35:
  - 用户目录 `/Users/lazyking/.agentpipeline/skills/` 尚不存在，属正常：内嵌技能开箱可用，
```

**要什么**

- `capture.mjs`：改成相对脚本自身位置解析，例如
  `const root = new URL('../design/', import.meta.url)`（或 `path.resolve(import.meta.dirname, '../design')`），
  `out` 同理指向 `.scratch/shots`。行为不变，去掉本机用户名与本机仓库位置。
- `03-config-landing-docs-and-tests.md:35`：`/Users/lazyking/.agentpipeline/skills/` → `~/.agentpipeline/skills/`
  （这是历史票面记录，改这一处措辞即可；其余正文不动）。

**为什么**：票 11 的三类检查里「本机绝对路径」这一类，全仓 25 处命中里 **只有这两处是真实的本机路径**
（其余 23 处是 `/home/u/...`、`/home/t/...`、`/home/me/...` 这类合成测试夹具，以及
`ProjectForm.svelte:67` 的泛用占位符 `/Users/you/code/project`——均已判定「留」）。
`capture.mjs` 与那份票面是**已入库文件**，不处理会随仓库一起公开出维护者的用户名与仓库绝对位置。

**补充**：`.scratch/shots/capture-pixel.mjs` 经检查**没有**绝对路径命中，不需要动。
