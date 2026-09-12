# 05: 工具子系统（内置工具与文件策略）

**What to build:** agent 可用的内置工具集及其执行器：read/write/edit/delete/list_dir/run_command/submit_metadata 真实执行；文件工具路径策略（workdir 边界 + deny 清单 + symlink 逃逸拒绝）；工具结果超阈值卸载到文件；命令输出脱敏。

**Blocked by:** None（can start immediately）

**Status:** done（已实现）

- [x] 7 个内置工具真实执行，阶段感知写根；未知工具校验拒绝（spawn_sub_agent 被拒，决策 148⑦）
- [x] FileToolPolicy：deny 先于 allow、realpath 解析、symlink 写拒绝（决策 104；shell 不受限，G11 靠审计）
- [x] offload_threshold_tokens 单一阈值卸载，stdout_path 落 kanban_node_commands（决策 110）
- [x] stdout/stderr 脱敏先于回填（决策 118）
- [x] 缺口（归票 17）：run_command 未传播真实 pgid
