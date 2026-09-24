# 04: 配对的 403 带一个机器可读的 `kind`——界面不再按中文字样分支

**What to build:** `Talk.svelte:521-523` 的 `needsPairing` 按后端报文的**中文字串**分支
（`message.includes('还没配对')`），用来决定挂不挂配对入口。这条耦合要拆掉，但**拆法是一个
后端裁决，不是前端重构**——本票先把这个裁决摆出来。

**为什么它至今只能看报文**（`:519-520` 的注释，本票不动它）：403 在本应用里被**两处**用着——
配对缺失与跨源防护（决策 128）都会 403。只看状态码会把「Origin 不对」也挂上配对入口，
而那是一条走不通的指引。

**查证过的事实**：生产者是 `crates/app/src/stream.rs:107`
（`ApiError::forbidden("这台设备还没配对：请在跑服务的电脑本机打开手机访问页扫码")`），
而 `crates/app/src/state.rs:400-407` 的 `ApiError::forbidden` 设的是 **`kind: None`**——
与技能市场那八类失败（决策 194 裁决⑦，`kind` 机器可读、界面按它分支）**不同姿态**。
**故今天确实只有报文可依。**

**同一个反模式在别处也被点过名**：`api/client.ts:154-156` 明写「按它（`kind`）分支，
不按状态码、**更不按 `message` 里的字样**」——规则已经存在，而 `Talk.svelte:521` 与
`realtime/foreman.ts:273-275`（`isTimeoutMessage` 的 `startsWith('请求超时')`）两处违反它。

**Blocked by:** None（可立即开始，但**需要一次裁决**）

**Status:** done（2026-09-23；用户裁决取 (a)，实现与决策 259 同批落地——见交付说明）

- [x] **裁决点**（三选一，先定后做）：
  - (a) 给配对的 403 加 `kind`（如 `pairing_required`），与技能市场八类同一姿态；
    界面改按 `kind` 分支，报文原样留着给用户看
  - (b) 不加 `kind`，至少把 `needsPairing` 从路由件搬进 `lib/sharePairing.ts`
    （它已经处理配对的另一半：`Share.svelte:90` 按 `err.status === 403` 分类）——
    **这只是把耦合换个位置并给它一个单测**，不消除耦合
  - (c) 明确不做，就地留一条注释说明「配对 403 无法与跨源 403 区分，故按报文分支是**当前唯一解**」
- [x] 若选 (a)：后端 `stream.rs` 的配对拒绝带 `kind`；`ApiError::forbidden` 需要能收 `kind`
  （今天是硬编码 `None`）；前端 `needsPairing` 改吃 `kind`
- [x] 若选 (a)：**注意通道**——这个错误到界面时是一条**流错误字符串**
  （`Talk.svelte:1099` 的 `failForemanStream(stream, failureNotice((err as Error).message))`），
  不是 `ApiError` 对象。故 `kind` 要么随流错误一起传（`ForemanStreamState.error` 今天只是
  `string | null`），要么该判据改在 `ApiError` 还在手上的那一层判。**这一步是本票真正的工作量**
- [x] 无论选哪个：`realtime/foreman.ts:273-275` 的 `isTimeoutMessage` 是**同一反模式的兄弟**
  （`startsWith('请求超时')`，而那句由 `api/client.ts:95-101` 构造）——**本票不做它**，
  但裁决时一并记录，决定是否另立一票
- [x] 补测：选 (a) 则 `needsPairing` 的 `kind` 分支各一条 + 「跨源 403 **不**挂配对入口」一条
  （后者是 `:519-520` 注释守着的那个教训，必须钉住）
- [x] 若本票产出新裁决，`docs/decisions.md` 追加一条

## 交付说明（2026-09-23）

**裁决**（用户，2026-09-23，AskUserQuestion 两问）：**(a) 加 `kind`**；`isTimeoutMessage` 兄弟**另立票 06**。
裁决落成 **决策 259**（`docs/decisions.md`，同批追加）。

实现要点与偏离：

1. **后端比预想的便宜**：`ApiError` 本就有 `kind` 字段与 `with_kind()`（决策 194⑦ 的遗产），
   故只是在 `stream.rs` 把两处拒绝抽成 `pairing_rejected()` / `origin_rejected()` 两个构造函数、
   前者带 `KIND_PAIRING_REQUIRED`（照 `KIND_SKILL_NOT_FOUND` 常量先例）。抽函数是为了让
   「带 kind + 报文原样 / 跨源不带 kind」能被**单测**钉住——中间件本体要真请求才跑得到
   （该文件自己的注释也说「真实中间件行为在 L3 契约测试里用真请求验证」）。
2. **票面点名的「通道」就是全部工作量**：错误到 Talk 时是 `failureNotice(err.message)` 字符串，
   `kind` 已丢。判据落在**两处 catch**（读会话 / 发话），趁 `ApiError` 还在手调
   `lib/sharePairing.ts::isPairingRequired(err)`，产出两枚布尔（`loadErrorPairing` /
   `sendPairingNeeded`）；`buildTurns` 的输入从 `needsPairing` 回调**收窄为布尔** `pairingNeeded`
   （票 02 刚定的输入类型随之改，`talkTurns.test.ts` 同步）。
3. **测试**：`stream.rs` 3 条 + `sharePairing.test.ts` 4 条（含票面点名的「跨源 403 不挂配对入口」
   与一条「报文写着还没配对但 kind 不是 → 不挂」的字样 spoof）+ `talkTurns` 字样 spoof 1 条 +
   `delegation-scan` 守卫 1 条（Talk 无 `includes` 谓词、lib 判 kind、buildTurns 收布尔）。
4. **已知缺口（如实记）**：L3 真请求级的配对 403 断言**未加**——现有集成测试都在
   `tests/integration/api_contract.rs`，那是并发会话在飞的文件，本批不碰。生产者形状由单测钉、
   消费端由前端测试钉，中间「真请求过中间件」这一段暂无自动化。
5. **明确不做**（已进决策 259）：`Share.svelte:90` 的 status 分类、`routes/pairing.rs:32` 的
   「令牌只能本机读」都不带 kind——两者的语义都不是「设备未配」，带上会把别的 gate 误报成配对入口。
