# 18: 详情页中间档折行 + hero 轨道不再撑破页面

**叠:** A（不动规格）

**来源:** R2-05 的 5a / 5b；由 [07](07-narrow-band-layout.md)（决策 215）开出

**What to build:** 实测详情页在 480–819px 之间主栏被挤成一条，`hero` 轨道从约 830px 起
把整页撑出横向滚动：

```
w=480 → main 102, pageOverflow 352      w=520 → main 142, pageOverflow 312
w=600 → main 222, pageOverflow 232      w=768 → main 390, pageOverflow 64
heroScrollW 812 恒定 vs heroClientW 390(w=768) / 102(w=480)
```

**Blocked by:** 07（阈值与档位由它定，实现照抄，不再另定）

**Status:** wontfix（2026-10-01 用户裁决收掉：详情页中间档折行不再做；如真实使用在 480–819px 再疼，按决策 215 的断点表另立票）

- [ ] `.detail.split` 改成 `≥1100px: minmax(0,1fr) 320px` / `820–1099px: minmax(0,1fr) 280px`
      / `<820px: 单列`（`display:block`，左栏下限 `minmax(480px,1fr)`）
- [ ] `.rail.hero` 容器内横向滚（`overflow-x: auto`）：**不裁切、不把滚动传给文档**
      （`.rail.spine` 的裁切语义不动）
- [ ] 档案盒落到主栏下方时，sticky 与动作行照旧（六个 e2e 文件依赖动作行定位）
- [ ] e2e：宽度扫描断言 `main >= 480`（820–1099 档）/ 单列时主栏 = 容器宽、
      `document.documentElement.scrollWidth === clientWidth`（页面不横向滚）、
      `hero.scrollWidth >= hero.clientWidth` 且 hero 自己的 `overflow-x` 是 auto
- [ ] 回归：`≤479px` 移动款形态（纵向脊线 + 底部动作坞）一字不变

**边界.** 不动视觉规格；不动移动款那一档。
