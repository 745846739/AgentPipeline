# GitHub 作为技能来源（取代自定 registry）

来源：2026-09-16 的诉求「如果要改成从 github 下载，是否具备可行性」→ 三轮实测 → 经**拷问 Q1–Q18**
逐条裁定。结论是**可行，且它把所有其它选项要付的代价全省了**；而它要求的不是"再接一个来源"，
是**把自定 `/index.json` 那一层整个换掉**（决策 194）。

实测底稿（数据与复现命令、以及"当时推荐 vs 最终裁定"的差异表）见 [`background.md`](background.md)。
**读推荐之前先读那张差异表**——里面有几条推荐已被后续事实推翻。

## 本目录的票

| 票 | 内容 | 阻塞于 | 状态 |
|---|---|---|---|
| [01](issues/01-repo-reader.md) | 仓访问层与测试接缝（libgit2 + 离线 fixture，不含界面与安装） | — | ready-for-agent |
| [02](issues/02-install.md) | 从一个钉住的 commit 安装（八类失败、来源记录、一键安装连带） | 01 | ready-for-agent |
| [03](issues/03-browse-ui.md) | 浏览与安装页（仓名单两级、冷启动名单、分组列表、SHA 刷新） | 01、02 | ready-for-agent |
| [04](issues/04-remove-old-layer.md) | 拆除旧层（代码 / 界面 / 测试 / 文档 + 决策 194 的生效确认） | 02、03 | ready-for-agent |

**01 起步，02 与 03 可并行，04 收尾。** 04 排在最后不是礼节：它删掉的是**当前唯一能用的那条路**，
必须等新层建好并有用例守着再动手。

## 裁定要点（决策 194）

- **只走 git 通道**：`ls-remote` 取 tip → `depth(1)` fetch → 直接读对象库，不落工作区。一个 origin
  （`github.com`），不引 API 配额，不引第二个与第三个 origin。
- **锚是 commit SHA**，由 libgit2 在 fetch 时本地校验对象哈希——比"下载字节的 sha256"更强，且**不需要
  任何索引来托管摘要**（正好把最脆的一环从关键路径上移开）。
- **信任单元从 origin 换成 `owner/repo`**：GitHub 模式下 origin 恒为 `github.com`，按 origin 放行等于
  放行全世界任何作者的任何仓。
- **列表钉住浏览时的 commit**：看到的 = 装到的；界面显示"列表基于 `<短 SHA>`（时间）"。
- **引擎零改动**：落盘侧（`SkillPackage` / `install` / 同名冲突 / 路径穿越）与装前预览、信任标记一行不动；
  深度无关的"技能识别"判据落在**新增的扫描层**，不改解包层。
- **镜像、私有仓不进第一版**；商业目录的专用适配器、签名与人工审核队列**不立票**。

## 关键约束

- 任何打真 GitHub / 真镜像的测试**必须 opt-in**（`test.skip` + 环境变量，照
  `frontend/e2e/screenshots.spec.ts` 的先例），不得进默认质量门。直连实测本来是通的，但**会偶发中断**
  （本 effort 期间复现过一次 75 s 连不上）——把它放进默认门就是给自己埋 flaky。
- 离线 fixture **必须两层**且都在默认门内：本地裸仓（快单测，但**不支持 shallow**）+ 离线 smart HTTP
  （覆盖 shallow 与传输策略）。传输与来源判定要纯函数化单测——离线 fixture 打不到"仓不在白名单"那类判定。
- 每票验收须落在既有闸门内：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`；涉及前端加跑 vitest / svelte-check / build / 离线 E2E。
- **迁移文件不可改**（决策 193 的教训）：`0009_market_sources.sql` 的建表语句保留，只删读写它的代码。
