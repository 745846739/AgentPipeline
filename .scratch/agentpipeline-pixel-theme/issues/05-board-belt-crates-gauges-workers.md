# 05: 看板传送带 + 货箱 + token 量表 + boss 条 + 列头小人

**What to build:** 看板从「一列列卡片」变成「一条还在运转的流水线」。列头是工位（8×8 sprite +
工位名 + 挥锤小人），列与列之间是传送带（信号灯 + 链节，运行段离散步进），每张卡是一个货箱
（2px 描边盒 + dither 顶盖带 + 硬投影 + 16 段 token HP 条；运行中另带 20 段 boss 战尝试条；
pending 是琥珀急停）。这是本 effort 最大的单票，也是用户日常盯得最久的一屏。

**Blocked by:** 03 全站像素控件与基元

**Status:** ready-for-agent

- [ ] **轨道脊线 → 传送带**：字符线路行（`○ ● ◆`）与字符站点一律移除，替换为像素灯与链节；
      传送带 8px，`repeating-linear-gradient` 画像素链节；站点 = 12px 信号灯方块 + 工位名
      （12px + `letter-spacing: 0.08em`）；运行中工位两侧链节以 `steps()` 离散步进
- [ ] 8 列列头挂 8×8 工位 sprite（flag / gem / hammer / flask / gear / lens / shield /
      merge / trophy 按列对应）+ 工位名 + **挥锤小人**（run 绿 0.6s 快挥 / wait 琥珀 1.8s 慢挥 /
      idle 灰静止；帧切换为离散 opacity 翻转）；列头计数保留；sprite 用 `currentColor`
      随列头状态、`shape-rendering: crispEdges`
- [ ] 并行区间（develop-design ∥ test-design）画成双带并轨、在 develop 前合流；
      折返线画为回流带虚线，出现打回任务时短暂点亮为红（**sync-check 仍不占站、不出现在任何轨道**，决策 107）
- [ ] **货箱卡**：2px 描边盒 + 6px dither 顶盖带 + `4px 4px 0` 硬投影；hover = 描边亮一档；
      一张货箱 = 一枚灯 + 一种描边色（pending 琥珀 / failed 红 / 其余 pane）；queued / waiting 无灯灰字
- [ ] meta 行带 **16 段 token HP 量表**（每段 5×10px、间隙 2px，满格 ≈ 64k tok，颜色随状态灯）；
      大数字 24px 起步配 12px 灰注
- [ ] 运行中货箱显示 **boss 战尝试条**（20 段，`已用尝试 / 上限` 折算点亮段数，
      最后一次尝试整条转红——只是放大版量表，不引入第二套数据）
- [ ] pending 货箱 = 琥珀描边 + `?!` 灯 + 琥珀 2px 左缘条；stalled 整卡琥珀 + 滞留角标保留
- [ ] **修既有 e2e 断言**：`concurrent.spec.ts` 的 `.head-label` 文本等于 `['[dev]','[test]']`
      改为断言分支徽章的分支身份（`--branch-dev` 蓝 / `--branch-tst` 紫 + 文字双编码），
      断言语义而非方括号——与决策 84 的标注对应
- [ ] 空列空态文案保留（决策：不放插画，一句话）
- [ ] 卡片**仍禁拖**；整卡导航链接与动作按钮的层级关系**保持决策 164 的修复结果**
      （动作按钮必须可点，不得回退）
- [ ] vitest 补：货箱渲染出 16 段量表与正确点亮段数；boss 条 20 段且最后一次尝试整条转红；
      列头小人按状态取到 run / wait / idle 节奏；sprite 组件按名字渲染非空 SVG 且用 currentColor
- [ ] 既有 vitest / playwright 全绿
