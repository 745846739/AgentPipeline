# 管线工具规格表（pipeline-tool-catalog）

**Status:** ready-for-agent

> **来源**：2026-09-30 架构体检 ③ 号卡 + 拷问定案。决策落 `docs/decisions.md` 353。
> 值班长 `ForemanToolSpec`（全仓最好的深模块）的模式推广到管线 8 个内置工具。

## Problem Statement

`model_request.rs` 给每个内置工具广告的是空壳：

```rust
description: String::new(),
parameters: serde_json::json!({"type": "object"}),
```

`edit_file` 的 old_text/new_text 契约在 `execute()` 有硬校验与报错信息，却不在广告出的接口里——
模型只能从 prompts.rs 的散文猜参数形状，两处知识会漂移。「一个工具是什么」散在五处
（client.rs 名单、tools.rs 层名单 + dispatch、model_request 的 tool_defs、prompts 散文），
加一个工具要碰 5 个文件。

## Solution

新文件 `crates/core/src/agent/catalog.rs`：8 个内置工具的 name + description +
JSON-Schema parameters 一张表，喂饱 `model_request::tool_defs` 与 tools.rs dispatch。
三份层名单（`ENV_TOOLS` / `ENV_WRITE_TOOLS` / `SERVICE_WRITE_TOOLS`）保留不动，
加冻结断言钉「目录名字集 = 三份名单并集 + 分层」。

## Constraints

- 不重引手标层枚举或层谓词——决策 247 删层字段的理由同样适用于谓词
- D 层适配器不动（决策 357：ServiceToolRunner 不做）
- 加一个工具的触碰面从此 = catalog.rs 一行

## 执行顺序

全盘点第三批。
