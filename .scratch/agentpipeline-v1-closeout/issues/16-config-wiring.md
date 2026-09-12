# 16: 配置面接线

**What to build:** 两处「文档承诺、代码不认」的配置缺口：① `PromptsConfig.dir` 未接入——`prompts_root` 已实现并接受该覆盖，零调用方，实际永远读 `{home}/prompts`；② `[logging]` 的键与结构体根本对不上——文档写 `format` / `file`，结构体是 `level` + `json_file`，于是文档承诺的 JSON / 文件日志既配不出也落不了地。接线并让键名与文档一致（含旧键的兼容或明确废弃说明）。

**Blocked by:** None (can start immediately)

**Status:** done

- [ ] `prompts.dir` 生效，覆盖时从该目录读模板 / persona，缺省仍回落 `{home}/prompts`
- [ ] `[logging]` 的键名与 `docs/agents.md` 文档一致，且 `format` / `file` 真正生效（含文件日志目录创建与权限）
- [ ] 未知或冲突键的处理姿态明确（沿用既有配置校验姿态，不静默忽略）
- [ ] `docs/agents.md` 配置示例与实现一致；若决定废弃某键，文档显式标注
- [ ] 用例覆盖：prompts 目录覆盖生效；日志 format / file 生效（文件被创建且内容为该格式）
