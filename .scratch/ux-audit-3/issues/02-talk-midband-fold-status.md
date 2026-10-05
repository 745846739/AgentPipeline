# 02: 对讲台 480–899px 中间档对话列折行（现状核实）

**叠:** A（不动规格）

**来源:** A/ux-audit-2 票 [19](../ux-audit-2/issues/19-talk-midband-fold.md)
（**superseded**：被 `talk-mobile-space` 票 07 吸收并落地，决策 218①②⑤）

**What to see:**
第三轮重测对讲台宽度扫描（`[r3] ①.2`）：

| 视口 | `grid-template-columns`（`.talk`） | 右栏宽 | 页面横向溢出 |
|---|---|---|---|
| 1100 | `702px 340px` | 340 | 0 |
| 900 | `562px 280px` | 280 | 0 |
| 820 | `minmax(420px, 1fr) 280px` | **0（已是单列）** | 0 |
| 768 | `minmax(420px, 1fr) 280px` | 0 | 0 |
| 600 | `minmax(420px, 1fr) 280px` | 0 | 0 |
| 520 | `minmax(420px, 1fr) 280px` | 0 | 0 |
| 480 | `minmax(420px, 1fr) 280px` | 0 | **1** |

源码与实测吻合：`frontend/src/routes/Talk.svelte:3149-3153`（`max-width: 1099` →
`minmax(420px, 1fr) 280px`）、`:3166`（`max-width: 899` → `display: flex; flex-direction: column`
折行单列）、`:2353`（桌面款 `minmax(420px, 1fr) var(--dossier-w, 340px)`）。
**票 19 当年记的「480 → 82px 340px 一条缝」已不复现**：480 已是单列，右栏不占宽。

结论：**已修**（superseded 后确实落地，与第二轮 `[r2] ①.7` 逐格同值——900→280、
820 起单列——无回归）。与票 19 清单里的落点逐条对得上：三档阈值（≥1100 / 900–1099 /
<900）、对话列下限 `minmax(420px,1fr)`、右栏降到 280px。

**证据等级:** 实测（`[r3] ①.2` 数字 + 截图 `r3-talk-768.png` / `r3-talk-480.png`）
+ 代码（`Talk.svelte:2353,3149-3153,3166`）

**与前轮关联:** 现状核实（=前轮 19，superseded；取证点原样复现，未回归）

**建议:** 无。票 19 已由决策 218 落定，本轮只是把它钉在「已修」上。

**边界:** 审计票，不实现。探针里 `.talk-main` 选择器不存在（`mainW` 读 `null`），
是取证脚本的旁枝，不影响结论——判据看的是 `grid-template-columns` 与右栏宽。
