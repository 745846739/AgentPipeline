# notify-politeness：离线通知的礼貌两件上设置页

用户 2026-09-25：「把 imessage 推送相关的配置放到设置页」——追问下点名「静默时间等配置」。

病：通道那一半自决策 272 起能在设置页配、还能活生效；`cooldown_sec` / `quiet_hours`
只住 `config.toml` 的 `[notify]` 段，界面上只读展示。于是「你什么时候准吵我」——
半夜里最想改的那件——恰好是唯一改不动的：服务跑在别的机器上、或装的是打好的 dmg 时，
`config.toml` 不在手边。

裁决（决策 284，**显式修订 272⑥**）：设置页新增「礼貌」小节；礼貌两件作为**独立于
通道单元**的第二个单元（同表三列，同生同死），界面整体覆盖 `config.toml`，两组各自
报 origin、各交各的；保存即按解析后的礼貌重建出口（`NotifyPoliteness` 值对象）。
272⑥ 的「前端已有同语义的一份表」被**限定**而非推翻：那份表只管浏览器 toast。

## Tickets

- [01-politeness-settings.md](issues/01-politeness-settings.md) — 实现（core 值对象 +
  存储 + 两条端点 + 设置页小节 + 四层测试 + 文档）
