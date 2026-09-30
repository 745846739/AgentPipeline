# 01: 台账生命周期搬进 talk store，epoch 协议退场

**What to build:** 决策 354①——班次台账生命周期（reload / loadEarlier 游标翻页与滚动
位置保留 / switchTo / openFreshSession / submitArchive 归档坠落 / seen 标记
rememberLanding）从 Talk.svelte 搬进 `stores/talk.svelte.ts`。store 本就持有
sessionId / ledgerEpoch / 连接 / 哨兵定时器 / 队列，缺的只是台账这一块。
`bindRecalibrate` + `ledgerEpoch` 的 store→页面喊话协议、`markLedgerStale` 整段删除。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] store 新增：session / sessionList / loading / loadError / hasMoreEarlier /
      reload / loadEarlier / switchTo / 归档坠落 / seen 标记
- [ ] epoch/recalibrate 协议删除（喊话两端都没了）；折叠态搬运用到的
      `untrack` 重读点一并清理
- [ ] pending 详情指纹去重的 `$effect` 随台账归 store 管
- [ ] 行为零变化确认：e2e ㉖ 回看拼接 / ㉗ 滚到顶加载 / ㉘ 归档翻回照绿
- [ ] store 单测补台账生命周期（reload 合并已分页旧消息、loadEarlier 不滚动、
      归档坠落到下一班次）
