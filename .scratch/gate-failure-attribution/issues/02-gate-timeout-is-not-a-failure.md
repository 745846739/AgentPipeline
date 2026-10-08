# 02: repair 闸门把「超时」报成「没过」——两种终态共用一个形状

**Status:** needs-triage
**Blocked by:** None (can start immediately)

**What to build:** 作为值班经理，我要 repair `finish` 的闸门回执**把「超时」与「失败」分开说**。
今天它们同形，而后果是我（和值班长）**把一次冷编译当成了测试失败**——它已经让一轮 `finish` 白跑，
值班长还得自己写一句「实为测试二进制首编译耗时……**不是测试失败**」来给自己和人解释。

**What happened（2026-10-08，班次 `01M4CDY9EC9M9FSSE236GJXYZC` 的 407 轮）**：

那一轮第一次 `repair(action=finish)` 的回执原文：

```
闸门没过，没有出 diff、也没有提提议：闸门没过（test）：`cargo test --quiet` 退出码 -1，
用时 600013ms。完整输出在 {home}/worktrees/gate-output-repair-<id>-test.log
读数：lint 退出码 0、test 退出码 -1。改完再调一次 finish。
```

- `600013ms` 正好是配置项 **`test_command_timeout_sec = 600`** 的上沿；退出码 `-1` 是被**超时杀掉**
  （不是断言失败、也不是编译错误）；`lint` 退出码 0 说明代码本身没问题。
- 值班长紧接着在最终报告里写：「首次 finish 卡在 test 600s，**实为测试二进制首编译耗时**，
  缓存热后重跑即过——**不是测试失败**。」它自己把这条区分补上了，而回执里没有。

**为什么这条是错的方向**：`闸门没过（test）` 与真失败**逐字同形**。这个形状在这一族里已经有过
一次代价（`gate-failure-respam`，决策 388：一次 600s 超时被当成失败状态轮询，11 分钟推了 12+ 条
PWA 通知）。而「超时」在冷缓存下是**常态**——本目录 README 的止血实证里记着：`cargo test --quiet`
冷缓存约 **12.5 分钟**、暖缓存两条连跑 1m17s，两者都远在 600s 两侧。

**形状**：

1. repair 闸门的读数**区分三种终态**：通过 / **失败**（非零退出码或断言失败）/ **超时**
   （被 lint 或 test 那条时限砍断）。
2. 超时那一支的回执带「**超时**」字样 + 实测用时 + 阈值 + 一句「冷编译常见，可直接重跑一次」；
   失败那一支照旧报「没过」+ 退出码。
3. `gate-output-repair-<id>-<lint|test>.log` 的路径与「改完再调一次 finish」**保留**（它是模型
   下一步的唯一入口）。
4. **边界**：不动闸门跑哪些命令、不动 `test_command_timeout_sec` 的取值（把它调大是另一件事，
   而且它管着 lint 与 test 两条线）；**不碰流水线侧的闸门**——那是
   [01](01-gate-toolchain-and-failure-attribution.md) 的地盘。

**验收**：

- [ ] L2 集成：让 test 那条线必然超时（配一个小 `test_command_timeout_sec`，或让命令自己 `sleep`
      过线）→ 回执含「**超时**」且**不含**「闸门没过」
- [ ] L2 集成（反向）：真失败（非零退出码）→ 回执照旧报「闸门没过」并带退出码
- [ ] L2 集成：通过的路径一字不变（既有 repair 用例全绿）
- [ ] 手工面：拿本账班次那一次读数复跑——`cargo test --quiet` 冷缓存（约 12.5 分钟）刻意配成
      600s 线 → 回执应当说「超时」，而不是「没过」

**邻票关系（免得重开一族）**：`gate-failure-respam`（done，决策 388）点名过「`cargo test --quiet`
600s 超时」，但那是 **merge 闸门 + 通知去重**那条线；`gate-failure-attribution/01`（ready-for-human）
讲的是**流水线闸门**的环境归因与可见性。本票只动 **repair 闸门**的回执形状——三者共用的只有
「600s 这个数」与「失败形状会误导人」这两件事。
