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

**Status:** open

- [ ] 档案盒的 sticky `top` 取顶栏真实高度（CSS 变量或与 `--sbar-h` 同类的做法），不再写死
- [ ] 铭牌（`.dtag`）滚到顶时完整可见，不被顶栏覆盖
- [ ] e2e：1280 宽、滚动到档案盒吸顶 → 断言 `.dtag` 的矩形与顶栏矩形**交集为 0**，
      且 `.dtag` 顶部在视口内
- [ ] 回归：档案盒的动作行定位不变（六个 e2e 文件依赖）

**边界.** 顶栏在窄档可能折成两行而更高——用测量值或变量，别换成另一个魔数。
