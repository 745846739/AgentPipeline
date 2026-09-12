# 02: DB schema 与存储层

**What to build:** SQLite 持久层：§11.5 与 §12.4 的全部 12 张表（sqlx migrations 管理），以及存储操作——游标生命周期（分裂、skip-to-join、归档不删除、焦点投影同步）、任务与依赖（循环检测、准入计数）、阶段产出 upsert、冲突第一层比对、可观测性落库。

**Blocked by:** None（can start immediately）

**Status:** done（已实现）

- [x] 12 张表单一 migration，含部分唯一索引 uq_node_cursors_active_branch（决策 13/113）
- [x] split_cursors 原地改写 main + 插入分支、幂等，attempts 每游标独立（决策 80/82/90）
- [x] 游标行只归档不删除，node_runs 外键永不悬空（决策 113）
- [x] skipped_to_join 不改产出元数据（决策 93）
- [x] 冲突第一层：affected_files/new_symbols 键、yields_to 环消除、recheck（决策 71/102）
- [x] executor 乐观锁 try_claim/release + 启动残留清理（决策 36/127）
- [x] L2 测试（cursor_lifecycle / git_chain / scheduler_tick 三套件）锁定以上全部行为
