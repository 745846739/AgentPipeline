# 10: 指标页条形图

**What to build:** 全局指标页延续「轨道即导航」——不是 KPI 卡片横排，而是挂在轨道站点下的
分段条形图：`chart` 盒 + 链节 `plot` + 9 个站点列（标签 / 12px 灯 / 10px track 横条 / 数值），
灯与条同色。首过率没有数据时整条改用一句话说明，不画 0 冒充真实值。

**Blocked by:** 03 全站像素控件与基元

**Status:** ready-for-agent

- [ ] 指标页 = 台账基元 + `chart` 盒（2px 描边 + 硬投影）+ `plot`（绝对定位链节）+ 站点列
      （标签 / 12px 灯 / 10px track 横条 / 数值）；灯与横条同色（go / caution / stop / done /
      dev / test）；success rate、各阶段平均耗时 / 重试率 / validate 通过率、token 消耗全部照旧出一遍
- [ ] **首过率缺数据不画 0**：整条改用一句话说明（`frontend-design.md` §7）；其余指标照常呈现
- [ ] 站点数不缩不折；移动款 9 站放不进 430px 时 `plot` 横向滚动
- [ ] 数据源与字段映射逐字不变（`lib/metrics.ts` 与其既有单测不动）
- [ ] 既有 vitest（`metrics.test.ts`）与全部 playwright 全绿
