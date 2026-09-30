# 01: agent/catalog.rs 规格表 + 空壳广告退场

**What to build:** 决策 353——新文件 `crates/core/src/agent/catalog.rs`：8 个内置工具
（write_file / edit_file / read_file / delete_file / list_dir / run_command /
submit_metadata / Skill）的 name + description + JSON-Schema parameters 一张表。
`model_request::tool_defs` 改从目录表取（空壳 `description: String::new()` +
`{"type":"object"}` 退场）；tools.rs 的 25 臂 dispatch 名字引用目录表。冻结断言钉
「目录名字集 = `BUILTIN_TOOLS` ∧ `ENV_TOOLS` ∪ `ENV_WRITE_TOOLS` ∪ `SERVICE_WRITE_TOOLS`
的分层对应」。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] `agent/catalog.rs` 一张表：name + description + parameters（参数 schema 照
      `execute()` 的解析逐字段对齐，含 edit_file 的 old_text/new_text、run_command 的
      workdir 等全部必填/可选）
- [ ] `model_request::tool_defs` 从目录表生成广告集；工具顺序保持现役不变（provider 侧
      顺序敏感处先核）
- [ ] 冻结断言：目录名字集与三份层名单 + `BUILTIN_TOOLS` 的对应；断言写进 tools.rs 或
      catalog.rs 的 tests
- [ ] 抽查断言：每个工具广告出的 parameters schema 与 `execute()` 实际解析的字段一致
- [ ] prompts.rs 里与参数形状重复的散文段收敛（描述以目录表为准，散文留纪律不留形状）
- [ ] 验证：core 全量 + lint 绿；foreman e2e 与 executor e2e 照绿
