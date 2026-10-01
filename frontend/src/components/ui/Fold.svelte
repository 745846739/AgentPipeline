<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * 受控折叠块（决策 366）：收起行 + 展开体，现场与对讲台共用**同一套定义**。
   *
   * **为什么受控**：`<details open>` 交给浏览器自管的话，流式增量反复重渲染同一块时，
   * 人手动展开的那一块会被打回收起（决策 218② / 301）。这里由 `onclick` 里的
   * `preventDefault` 截掉默认翻转，展开态由调用方持有——两处调用点的存放处本就不同
   * （对讲台的折叠三表住 store，决策 315；现场的折叠表住组件作用域），所以这是个受控件。
   *
   * **为什么 `summary` 与 chevron 归这里**：`[open] > summary .chev` 这条旋向规则要跨过
   * 子组件边界，而 Svelte 5 的 scoped CSS 给选择器每个复合段都挂 `:where(.svelte-hash)`
   * ——父组件够不到子组件里的元素（决策 366 记的正是这个坑）。所以这两样由本组件持有。
   *
   * **字段排布留在调用方**：调用方在自己的 snippet 里放一层包裹元素（`.rcpt-head` /
   * `.retry-sum` …），`nm` / `args` / `rs` / `c` 那些子规则照旧锚在它上面，一条都不用改；
   * `flex` 档下这层包裹自带 `flex: 1`，把 chevron 顶到行尾。
   */
  interface Props {
    /** 展开态，调用方持有。 */
    open: boolean;
    /** 翻状态前先 `preventDefault` 掉浏览器默认翻转。 */
    ontoggle: (event: MouseEvent) => void;
    /** `box` = 左缘档位 + 底板（回执）；`bare` = 光秃的一行（过程组 / 轮壳 / 旧尝试）。 */
    chrome?: 'box' | 'bare';
    /** 收起行的流：`flex` = 一个占满的字段行；`inline` = 一行文字（对讲台的过程 / 思考）。 */
    row?: 'flex' | 'inline';
    /** 左缘档位（只 `box` 用得上）：`pending` 静置、`go` 成、`bad` 败。 */
    tone?: 'none' | 'pending' | 'go' | 'bad';
    /** 上边距的例外档（px）；缺省按 `chrome` 走（box 6 / bare 8）。 */
    space?: number | null;
    /** 调用方自带的类：`rcpt` / `retryfold` 这些既是站内语汇也是测试的选择器钩子。 */
    class?: string;
    title?: string;
    /** 收起行：谁、查什么、成没成。chevron 由本组件补在最右。 */
    summary: Snippet;
    children?: Snippet;
  }

  let {
    open,
    ontoggle,
    chrome = 'bare',
    row = 'flex',
    tone = 'none',
    space = null,
    class: klass = '',
    title,
    summary,
    children,
    ...rest
  }: Props & Record<`data-${string}`, string | number | undefined> = $props();
</script>

<details
  class="fold fold-{chrome} fold-{row} {klass}"
  data-tone={tone}
  style={space === null ? undefined : `margin-top:${space}px`}
  {open}
  {...rest}
>
  <summary class="fold-sum" {title} onclick={ontoggle}>
    {@render summary()}<span class="chev" aria-hidden="true">▸</span>
  </summary>
  {@render children?.()}
</details>

<style>
  /* 收起行：一抬手就到的一行——浏览器自带的三角标与列表符都撤掉，chevron 自己画
     （`▸` 是全站既有语汇）。展开提示常驻而非 hover 才现：触屏没有 hover，藏起来等于
     这一档的人看不到它（决策 301）。 */
  .fold-sum {
    color: var(--text-3);
    cursor: pointer;
    list-style: none;
  }
  .fold-sum::-webkit-details-marker {
    display: none;
  }
  .fold-sum .chev {
    flex: none;
    color: var(--text-4);
  }
  .fold[open] > .fold-sum .chev {
    transform: rotate(90deg);
  }

  /* flex 档：收起行是一个占满的字段行（字段那层包裹自带 `flex: 1`，chevron 随之贴到尾） */
  .fold-flex > .fold-sum {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  /* inline 档：收起行是一行文字（对讲台的过程 / 思考两处原本就是这个流） */
  .fold-inline > .fold-sum .chev {
    margin-left: 4px;
  }

  /* box 档：转述不是发言——左缘 4px 亮度阶 + 底板（与命令输出同一手法）。
     档位词只有一份：静置 --pane、成 --go、败 --stop。 */
  .fold-box {
    border-left: 4px solid var(--pane);
    background: var(--panel);
    padding: 6px 10px;
    margin-top: 6px;
  }
  .fold-box[data-tone='go'] {
    border-left-color: var(--go);
  }
  .fold-box[data-tone='bad'] {
    border-left-color: var(--stop);
  }
  .fold-bare {
    margin-top: 8px;
  }
</style>
