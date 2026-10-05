# 01: 详情页 480–819px 中间档主栏折行与 hero 横向溢出，现状如何

**出处:** 待填：`frontend/src/routes/TaskDetail.svelte` 的 `.detail.split` 断点与 hero 溢出规则行号 + 宽度扫描实测数字
**严重度:** 待填
**与前轮关联:** 待填（前轮票 [ux-audit-2/18](../../ux-audit-2/issues/18-detail-midband-fold.md)，wontfix）
**证据等级:** 待填

**证据占位:** 待填：480 / 600 / 768 三档读 `.detail` 的 `grid-template-columns`、主栏宽、
`document.documentElement.scrollWidth - clientWidth`、`hero.scrollWidth vs clientWidth` 与
`overflow-x` 计算值（截图落 `.scratch/ux-audit-3/*.png`）。
