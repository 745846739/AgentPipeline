# 02: 判 run 终态的同一处清执行权（显式修订决策 226）

**Spec:** `../spec.md`（Implementation Decisions 第 2 条）

**What to build:** 作为值班经理，我要 run 被看门狗判超时的**同一处**就释放执行权——
这样任务再也不可能因为「执行权没释放」而永久停住，**哪怕那个 future 永远不返回**。
这是这次故障的直接修复：原实现里执行权只在 `run_inner` 正常返回后才释放，
而看门狗判 timeout **只改了 run 行**，future 照样挂在阻塞的 `open()` 里，于是乐观锁
`WHERE executor_owner IS NULL` 永远为假，`resume` 重试 30 秒就永久放弃。

**Blocked by:** None (can start immediately)

**Status:** done（已实现，决策 302–311；四门 `make check` 全绿）

- [x] run 被判终态（超时 / 中止）的**同一处**清 `executor_owner`，与 `run_inner` 是否返回**无关**
- [x] 集成断言：造「future 不返回 + run 已判终态」的形态 → 执行权为 NULL → 紧接着的 `resume` / `try_run` **真的拿到执行权**
- [x] **反向断言**：健康在跑的任务（owner 持有、心跳在走、未超阈值）**不被误清**——这正是 `unstick` 文件头警告的那件事
- [x] 与启动恢复（清残留持有者、孤儿归队）语义不冲突，两条路不重复也不漏
- [x] **显式修订决策 226** 并落号：新增一格「执行权释放与 `run_inner` 返回解耦」；
      226 原裁的「不做 `AbortHandle`」「阻塞 syscall 收不到 `select!`」**原文保留不动**
- [x] 四门通过
