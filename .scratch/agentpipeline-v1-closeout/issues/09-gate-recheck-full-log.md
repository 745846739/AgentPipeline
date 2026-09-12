# 09: gate_recheck 注入完整命令日志

**What to build:** merge 闸门失败后 test 复检是一条**决策偏差**：决策 109 / spec §97 要求注入 `kanban_node_commands` 的完整日志 + 失败用例，当前注入的是命令与退出码加上首尾各 50/100 行的预览。改为闸门命令路径保留完整输出文件，复检段读全文并附失败用例。完成后被闸门打回的 test agent 能看到完整的失败现场，而不是被截断的尾部。

**Blocked by:** None (can start immediately)

**Status:** done

- [ ] 闸门命令的完整 stdout / stderr 落 `kanban_node_commands` 可读路径（不再只存预览）
- [ ] 复检注入段读完整日志 + 失败用例，不再使用首尾预览
- [ ] 注入体积仍有上界（超限时的截断策略需显式定义并在文档留痕，不得静默回退到预览）
- [ ] `gate_failures` 计数不被清零、闸门循环行为不变（决策 108）
- [ ] E2E-06a 断言注入内容含完整日志特征（例如被预览裁掉的中间行）；`docs/testing.md` §11 的决策 109 偏差条目更新为已关闭
