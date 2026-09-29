# 04: 设备清单 UI + 收尾验收

**What to build:** 设置页出现「已订阅设备」区块：列出每条订阅的创建时间与 UA，可单个撤销、一键全部清空，操作后清单当场变化；空清单给订阅引导而不是空白。收尾把文档补齐（testing.md 用例目录、decisions.md 新决策条目及其编号引用），最后跑通 spec 里的真机手动验收清单——这是整个 effort 唯一验真 APNs 的一票。

**Blocked by:** 03 (service worker + 订阅按钮), 01 (HTTPS 地基)

**Status:** UI 与文档 done（决策 323–327）；**真机验收未做**（依赖 01 的 106 切换，且那一步只有人能走）

- [x] 设置页「已订阅设备」区块：按时间列出 endpoint 摘要 / 创建时间 / UA；单个撤销后该行消失；全部清空后列表为空 —— `SettingsNotify.svelte` 的「订阅」一节；摘要 = `…前6…后6`（完整 endpoint 是一枚能力 URL，不回显）
- [x] 空清单显示订阅引导文案（照 settings 空状态既有先例），不显示空白区块 —— 「还没有设备订阅。在这台设备上点上面的『订阅此设备』——或者在手机上打开同样的地址（HTTPS）再订一次。」
- [x] 撤销/清空走既有配对令牌守卫端点，回环与局域网行为与订阅端点一致 —— 同一族（`PUSH_SUBSCRIPTIONS_PREFIX`）下的 `DELETE /notify/push/subscriptions/{id}` 与 `DELETE /notify/push/subscriptions`；§7 的守卫矩阵覆盖
- [x] docs/testing.md 用例目录表补入本 effort 的 L1/L2/L3/前端用例行 —— §5（`webpush.rs` 10 条 / `storage/push.rs` 7 条）、§6（`notify.rs` 7 条）、§7（`/notify/push/*` 6 条）、§9（前端 4 组 + e2e 一节）、§10（323–327 的 traceability，327 那行如实写「本票自动面为零」）
- [x] docs/decisions.md 追加新决策条目 —— 323（第四次通道扩面：突破 65 / 沿用 272 / 礼貌照 284 / 对 167 的定点加强）、324（出站失败只删 404·410）、325（SW 的分发形状与缓存纪律）、326（深链形状与「消费即抹」）、327（HTTPS 地基与**拆明文的挂起理由**）
- [ ] 真机手动验收清单通过：iPhone 装 CA → Safari 开 https 并添加到主屏 → 设置页切通道并订阅 → 清单出现该设备 → 触发一个 pending → 锁屏收到推送 → 点开直达那张卡 —— **未做**：前置的 HTTPS 切换被挂起（决策 327），且这一步只有人能走（装描述文件、加主屏、看锁屏）
- [ ] 全量质量门绿（`make check`：lint + test + frontend + e2e） —— **逐项实跑（2026-09-29 收尾）**：`cargo fmt --all --check` **全仓零 diff**、`clippy --workspace --all-targets -D warnings` **零告警**、`cargo test --workspace` **全绿**（core lib 650 / core 集成 462 / app 契约 216 / e2e 33 / 其余 70 + 40 + 3）、`npm run check` 0 错 0 警、`npm run build` 绿、`push-subscribe.spec.ts` 4/4 绿、全量 e2e **142 passed / 27 skipped**（27 条是两组 opt-in 取证套件，一直跳）。**两处红与本 effort 无关，且已在 HEAD（bc45be4）的干净 worktree 上逐条复现**：① `src/components/task/CommandLog.test.ts` 的「关键词过滤：命令行搜得到，换档后窗口游标回缺省」在**全量** vitest 里超 5s 时限（单跑该文件 10/10 绿；HEAD 全量同样 1 failed / 1025 passed）；② e2e `ux2-geometry.spec.ts` 的「档案盒吸顶时『等你拍板』铭牌不被顶栏盖住」在 1280×900 下重叠 6px（HEAD 同样红）。②的机制已量到：档案盒的 sticky 让位够不着——它的包含块没有余量（盒底与左栏底齐平在文档坐标 970），`top: calc(var(--topbar-h) + 16px)` 因此生效不了，盒在 86 而不是 94，铭牌（盒顶 −16）就压在 78px 顶栏下沿 6px 处。两处都**未在本 effort 里改**（改布局要单独立票 + 全量复验，且本 effort 的改动不碰那两处）。另：本轮闸门跑动期间同一工作区另有会话在编译/跑测（出现过 `deps/*.rcgu.o` 被清导致的链接失败、以及 e2e 在满载下把别的用例打成超时），凡是可疑的红都在**机器安静时单跑复验**过，上面的结论以复验为准
