# 01: 死掉的那一轮，半截字去哪——裁决③的「留白」与落地哨的清字打架

**What to build:**（先裁决、后动手——三选一见文末）决策 260 裁决③与
`realtime/foreman.ts::turnLanded` 的 doc 都写：刷新后跟着的那一轮**没换行而服务端也不再报
「在跑」**（进程被杀 / 重启——决策 223 明确不做那一轮的落账，台账里永远不会有它）
时，**「保留已经收到的半截字比清空诚实」**。而 `routes/Talk.svelte` 的落地哨是：

```ts
const landed = turnLanded(payload.messages ?? [], anchor) || !payload.turn_in_flight;
if (!landed) return;
session = payload;
followingSince = null;
stream = emptyForemanStream();   // ← 死掉的那一轮恰恰靠后半句进这个分支，半截字被清掉
```

`|| !payload.turn_in_flight` **不能摘**（摘了死掉的那一轮会永远跟下去），所以问题不在停跟，
在**停跟之后半截字摆哪**。现状是那一轮在时间线上**整段消失**——正是用户报的那条毛病
（「刷新就看不到实时对话流」）在死轮场景下的残留子集。

**裁决点**（三选一，先定后做；呈现方式是用户可见行为，照票 04 的先例先裁决）：

- **(a) 转成本地失败轮（推荐）**：`turnLanded=false && !turn_in_flight` 时走现成的
  `failForemanStream(stream, <新文案>)`——半截字原样留在时间线上、附一句说清发生了什么
  （姿态照决策 223 的 `FOREMAN_TIMEOUT_SUFFIX`：说清事实 + 下一步）。机制现成、可单测，
  需要定一句文案。
- **(b) 只停跟、不清字**：落地哨分两支——换行了才 `emptyForemanStream()`，没换行只把
  `followingSince` 置 `null`。改动最小，但流式轮停止跟随后**不再渲染**，半截字留在 state
  里用户看不见——与「保留」字面相符、与「诚实」的意图打折。
- **(c) 维持清空、修订决策**：认现状，决策 260 裁决③行内标注修订（「留白」改为「清空，
  以台账为准」）。代价：死轮场景用户看到那一轮凭空消失。

**无论选哪个**：判据落成纯函数（吃 `turnLanded` / `turn_in_flight` 两个读数，出「清 / 留 +
以何姿态留」），配单测钉三条——换行 → 清（台账拥有回话）；没换行且不在跑 → 按裁决处置；
仍在跑 → 继续跟。`delegation-scan` 或等价守卫钉 Talk 落地哨不再就地 `emptyForemanStream`
一把梭。

**裁决：取 (a)，且它不必再问用户**——本仓已经有一条更早、更强、且与本条同源的纪律把它定死了：
`realtime/foreman.ts` 的文件头（票 03 立）写着「断流时 `text` 就是**已经出现的文字**，
**任何一支都不许把它清掉**」，`failForemanStream` 的注释是同一条的落点。故 (b)「留着但看不见」
与 (c)「清掉」都与那条纪律冲突——**这不是三选一，只有一支立得住**。照既有纪律办，不问。

**Blocked by:** None

**Status:** done（2026-09-24；(a) 落地）

- [x] 裁决 (a)：死轮收成失败轮，半截字原样留着
- [x] 判据收成纯函数 `resolveFollowOutcome(rows, anchor, turnInFlight)`：keep / settled / lost 三支 + 单测 5 条
- [x] 文案 `FOREMAN_LOST_TURN_SUFFIX`（决策 223 超时后缀的同款姿态：说清事实 + 说清下一步；**不假装还能等**——与「仍在继续」那句说的是相反的实情）
- [x] 守卫：`lib/delegation-scan.test.ts` 2 条（落地哨按三支分派、死轮那一支取 `failForemanStream` 而非 `emptyForemanStream`；**牙齿检查**：改回清字即变红）
- [x] 记账：决策 260 裁决③行内订正 + `testing.md` + `glossary.md`（无需新号——这是同一条决策内部的订正，不是新裁决）

**来源：** 2026-09-24 talk-judgments 票 07 收口的两轴 code-review（Spec 轴）查出——
决策 260 自己的裁决③与实现矛盾。用户裁定该问题**不属票 07**（那是缺口记账票），单立本票，
归在途轮（决策 260）这批名下。
