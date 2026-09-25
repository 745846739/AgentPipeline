# 05: 调查——调用 Skill 工具后下一轮 prompt cache 全失

**What to build:** 一份结论报告（调查票，不在本票实施修复）。实测 run41 在调用 Skill 工具（grilling）后的下一轮请求 prompt 6,697、cache_read 仅 126——按现有设计 Skill 只返回普通工具结果、追加在对话末尾，前缀不该变，这个全失解释不通。查明根因：若属管线侧（序列化/前缀变化）给出修复票草稿；若属 provider 缓存行为，记录证据并关闭。

**Blocked by:** None（can start immediately）

**Status:** done（2026-09-25）——结论：**provider 侧缓存行为，非管线缺陷，关闭**（证据见 Comments）

- [x] 复现：构造带 Skill 工具调用的会话，调用前后各发一轮，记录逐请求 token / cache_read（以生产台账 `kanban_model_requests` 的逐请求读数代替重放——原始现场就在库里，无需重烧 token）
- [x] 定位：二分请求体差异（system prompt / 消息序列化 / 工具结果形状 / provider 缓存粒度）
- [x] 产出：结论 + 证据追加到本票 `## Comments`；**不附修复票**（管线侧无缺陷可修，见结论）

## Comments

- 来源：同一会话复盘——run41 seq10→seq11：prompt 4,910→6,697（增量恰为 grilling 技能正文约 1,787 token），cache_read 3,252→126。对照组 run39/42 同类工具结果追加时缓存近满命中，故疑点收敛在 Skill 调用这一步。

- **结论（2026-09-25 调查）：provider（`stealth/space-bunny-alpha`，openai 兼容族）的前缀缓存不可靠；run41 的「Skill 后全 miss」是巧合关联，不是管线缺陷。不立案修复。**

- **证据一：Skill 正文其实在 seq2 就进来了，且缓存正常命中。** 逐请求台账（run41，16 条请求）：seq1 2,207/2,205 → seq2 2,622/**2,232**（+415 ≈ 技能正文 1,784 字符的 token 数，满命中）——「调用 Skill 后下一轮」这个假设本身不成立；seq10→seq11 的两次大块进账是**两次 read_file 的 L2 卸载标记**（各 3,181 字符，读同一个 .txt），不是技能正文。

- **证据二：管线侧前缀不变量成立。** ① 四条 run（39–42）的 `prompt_template_hash` 全等（38cfc8f7…）——system prompt 逐字同源；② 一次 attempt 内 plan 冻结（`RequestPlan::assemble` 每 attempt 恰一次），messages 只追加——唯一的改写者是 L3 压缩，而 provider 窗口 128,000、软限 0.6×128k = 76,800，run41 全程峰值估算 ~17.5k，压缩不可能触发；③ seq11 的请求 = seq10 的请求逐字前缀 + [assistant(101 字符) + 卸载标记 tool_result(3,181 字符)]，纯追加。

- **证据三：seq11 的内容本身可缓存。** seq12 立即以 6,802/**6,695** 满命中 seq11 的全部内容——miss 不是「内容不可缓存」或「管线发了坏请求」，而是 seq10 那条缓存条目在 seq11 请求时**没有被匹配到**（126 = 全局公共前缀基线，即 §12.13.5 固定前缀 ≈126 token 的「全 miss 签名」）。

- **证据四（决定性）：同签名全 miss 在**与 Skill 毫无关系**的请求里大量出现。** 对讲台值班长会话（`run_id IS NULL`，纯聊天、零工具、零管线机制）314 条请求中 **21 条** cache_read 恰为 125/126；流水线 run42（成功那轮）自己在 seq5（10,588/**2,621**）与 seq6（19,637/**4,184**）也两次轮中 miss，seq7 起恢复满命中。miss 的出现与「是否调用过 Skill」无相关，与「会话内容是否变化」也无相关——provider 侧的缓存条目匹配/驱逐本身不稳定。

- **判定与处置：** 按票面「若属 provider 缓存行为，记录证据并关闭」——关闭，不立修复票。管线侧能做的（resume/retry 前缀稳定：决策 278/279；流式护栏：决策 280）已在本批票 01–04 落地，我方不再制造打穿；provider 侧的偶发 miss 表现为多付一次输入 token，不改机制（与决策 280「不换模型/采样参数」同一条姿态）。若日后换 provider 或升级网关，此现象应随之消失，可用本票的四条证据复验。
