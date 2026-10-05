# 03: 不可逆动作单击即发、无确认步、量级与后果反着来（现状核实）

**叠:** A（不动规格——规格由 ux-audit-2 票 [14](../ux-audit-2/issues/14-destructive-confirm.md) / 决策 216 定）

**来源:** A/ux-audit-2 票 [21](../ux-audit-2/issues/21-destructive-confirm-impl.md)
（2026-10-01 以 **wontfix** 关闭；决策 216 保留为「怎么定」的记录）

**出处:** `crates/core/src/actions.rs:229,243,252,272,278`；`frontend/src/components/board/PendingActions.svelte:115,131,143`；`[r3] ②.1`；截图 `r3-merge-first-click.png`

**严重度:** 高（不可逆动作单击即发、无确认步——误触即不可撤销）

**What to see:**
在真页面（pending = `merge_approval` 的详情页）点第一档动作，量点击前后的 DOM 与按钮量级
（`[r3] ②.1`）：

```
点击前   buttons = [ {text:"返回修改", cls:"btn svelte-…"},
                     {text:"合入",     cls:"btn solid svelte-…"} ]
「合入」钮数量 = 1
第一次点击后  buttons = []   confirmTexts = []   bodyHasConfirm = false
```

即：**第一次点「合入」直接提交**（点击后动作区整块消失，进入已提交态），**没有任何
「确认…？」+ 同一颗钮 + 取消 的两步确认**——票 21 要的那条内联确认步**没有**。

量级：`合入` 是 `btn solid`（实心主按钮），`返回修改` 是 `btn`（描边）——这与票 21 描述的
「量级与后果反着来」一致的下半句；destructive 的 `终止任务` 探针在本 harness 造出的
`merge_approval` 态下**不存在**（`[r3] ②.2 {"found":false}`，那是 `retry_exhausted`
/`dependency_failed` 态才有的旁路动作，见 `crates/core/src/actions.rs:229,243,252,272,278`），
故本轮**未**在真页面量到 `终止任务` 的描边色，标「未验证」。

结论：**有意不做（wontfix：2026-10-01 用户裁决收掉，本轮只记现状核实，未重开；无回归）**。决策 216 定的「三档量级 + 内联两步确认」
在生产代码里查无实现：`lib/actions.ts` 无 `actionTier`，`PendingDossier.svelte` /
`PendingActions.svelte` / `DiffReviewPanel.svelte` 的动作钮只有 `solid`（恢复动作）与
`quiet`（旁路/等待）两档，没有 `gate-skip` 的琥珀描边、也没有 `destructive` 的红描边。

**证据等级:** 实测（`[r3] ②.1` + 截图 `r3-merge-first-click.png`）+ 代码
（`crates/core/src/actions.rs:229,243…` 的 `终止任务` label；`PendingActions.svelte:115,131,143`）
—— `终止任务` 按钮量级一处标 **未验证**（harness 该态下无此动作）。

**与前轮关联:** 现状核实（=前轮 21，wontfix；无一键即发形态变化，未回归）

**建议:** 维持 wontfix。重开判据照票 21：`actionTier(action)` 纯函数（不靠标签文字匹配）+
三档渲染类 + 四类动作（`合入` / gate-skip / `cancel` / `重置配对`）接内联两步确认。
本轮**只记现状**。

**边界:** 审计票，不实现；不给具体实现细节。
