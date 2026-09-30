# 09: 档案盒 sticky 偏移改用顶栏真实高度

**叠:** A（不动规格）

**来源:** R2-11（实测）

**What to build:** 滚动待拍板任务时，档案盒的「⏸ 等你拍板 · …」铭牌被顶栏吃掉。
实测（1280×900，滚 500px）：

```
topbarH=78   dossierTop=56   tagTop=42   headerBottom=78   stickyTopRule="56px"
```

档案盒 `top:56px`（`PendingDossier.svelte:227-229`），而顶栏实测 78px 高且不透明
（`TopBar.svelte:209-215`）：顶栏压住档案盒上沿 22px，**包括整块 `top:-16px` 的琥珀铭牌**——
铭牌正是「为什么这里有东西等你」的那一句话。

应用别处知道顶栏是 78–81px（`Talk.svelte:1372-1373` 的注释），56 是个没对上的旧魔数。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] 档案盒的 sticky `top` 取顶栏真实高度（CSS 变量或与 `--sbar-h` 同类的做法），不再写死
- [x] 铭牌（`.dtag`）滚到顶时完整可见，不被顶栏覆盖
- [x] e2e：1280 宽、滚动到档案盒吸顶 → 断言 `.dtag` 的矩形与顶栏矩形**交集为 0**，
      且 `.dtag` 顶部在视口内
- [x] 回归：档案盒的动作行定位不变（六个 e2e 文件依赖）

**边界.** 顶栏在窄档可能折成两行而更高——用测量值或变量，别换成另一个魔数。

## 实施记录（2026-09-18）

**落点**

| 处 | 改动 |
|---|---|
| `src/components/layout/TopBar.svelte` | `bind:offsetHeight={topbarH}` + 一个 `$effect` 把实测值写到 `document.documentElement` 的 `--topbar-h`。用 `offsetHeight`（**含** 2px 下框）而不是 `clientHeight` |
| `src/app.css` | `:root { --topbar-h: 78px }` 作兜底（首帧、或 JS 未跑到时） |
| `src/components/task/PendingDossier.svelte` | `.dossier { top: 56px }` → `top: calc(var(--topbar-h) + 16px)` |

**为什么 +16px**：铭牌 `.dtag` 自己 `top: -16px` 向上压在框沿上——只让位顶栏高度，
那 16px 的铭牌照样钻到顶栏底下。这一条写进了 CSS 注释。

**一句话说明为什么不换成另一个魔数**：顶栏在窄档会折成两行而更高（移动款约 138px），
写死任何值都只是「在某一档对」；量出来的值在每一档都对。

**证据**：`frontend/e2e/ux2-geometry.spec.ts` ③「档案盒吸顶时『等你拍板』铭牌不被顶栏盖住」
——1280 宽滚动到吸顶，断言 `.dtag` 矩形与顶栏矩形交集为 0 且铭牌在视口内。

## 追记（2026-09-30，决策 337）：取样点修了，让位规则一个字没动

CI 首跑红在这条用例上（铭牌与顶栏相交 6px），**不是让位算错**——探针实测吸顶是好的
（滚 22–140px 恒为 `dossierTop=94 / tagTop=80`，而顶栏下沿 78）。红的是**取样点**：这一页
夹具只能滚 156px，上面那句「滚 500px」被夹到 156，采到的是 sticky 的**行程末端**——行程受
包含块（网格行）的下沿所限，滚到头之后右栏随页往上走（`dossierTop` 掉到 86），铭牌跟着上移
8px 才相交。**修法**：位置算在钉住之后、离场之前（`natural - pin + 90`），并**先断言确实吸顶**
（`abs(dossierTop - 钉位) <= 1`）——验收条件「滚动到档案盒吸顶」这句此前没有一条断言真的验它。
三条原判据（交集 0 / `tagTop >= headerBottom - 2` / 铭牌在视口内）一个字不改。
