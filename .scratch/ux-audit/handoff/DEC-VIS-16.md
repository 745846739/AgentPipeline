# DEC-VIS → T（票 16 / 决策 196）：契约新增 `boardPinBreakpoint`

**目标文件**

- `frontend/src/theme/contract.ts`
- `frontend/src/theme/contract.test.ts`

**要什么**

1. `GEOMETRY` 新增一条 `boardPinBreakpoint: 1400`，注释写清：**桌面款看板钉右档的媒体断点**
   （≥1400px：`merge` / `done` 钉右、中间六列横滚；<1400px 不钉右）；CSS 的 `@media` 不认
   自定义属性，所以 `Board.svelte` 里它是字面量 `1400px`，两者相等由
   `frontend/src/lib/pipeline.geometry.test.ts`（B 的票 17 新增）**静态扫描断言**。
2. `contract.test.ts` 里那条 `it('唯一媒体断点 480px（不新增中间断点）')` 的名称 / 注释要改写：
   决策 196 起断点有两条（480px 移动款 + 1400px 桌面款内部的钉右档）。断言
   `GEOMETRY.mobileBreakpoint === 480` 本身照旧，但**不能再写「唯一」**——否则这条用例的
   名字与规格自相矛盾（旧口径已由决策 196 显式开例外，并在
   `.scratch/agentpipeline-pixel-theme/spec.md` 的 Out of Scope 那条就地标注）。

**为什么**

决策 196（`.scratch/ux-audit/issues/16-decision-board-overflow.md`；规格落点
`design/theme-6-pixel.md` §2.7）定下宽屏钉右与阈值 1400px。几何常量按契约模块的职责
（token / 几何 / 状态映射的唯一事实源，决策 169）应当在契约里有一份，票 17 的看板实现
与几何守护都从它读值——但票 17 的实现方（B）名下没有 `contract.ts` / `contract.test.ts`，
按 `parallel-brief.md` §一.1 走交接。

**口径来源（照抄别改）**：`design/theme-6-pixel.md` §2.7；决策日志行
`.scratch/ux-audit/.rows/196.md`。
