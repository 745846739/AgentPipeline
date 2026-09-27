<script lang="ts">
  import { theme } from '../../stores/theme.svelte';

  /**
   * `boxed`：按按钮材质装盒（描边 + 面底 + 按压位移）——设置落地页页头用；
   * 不带则沿用状态行里那枚的裸样式（它坐在底栏上，再套一层盒是双重描边）。
   */
  let { boxed = false }: { boxed?: boolean } = $props();
</script>

<!--
  深浅切换钮（决策 169 / theme-6-pixel.md §2.1 夜班靛、§2.4 掌机背光）。

  两处挂载、一份实现（决策 300）：
    · 桌面档底部状态行（`StatusLine.svelte`，裸样式）；
    · 设置落地页页头（`SettingsLanding.svelte`，`boxed`）——窄档状态条整条退场后，
      手机端**只有这一个入口**，从底部「设置」页签一进落地页就看得到。
  形态与文案（`浅色` / `深色`、`aria-label="切换到…主题"`、title）逐字沿用状态行那一枚，
  换个位子不换词——`ux2-geometry.spec.ts` 靠这个可访问名找到它。
-->
<button
  type="button"
  class="theme-tog"
  class:boxed
  onclick={() => theme.toggle()}
  aria-label={theme.current === 'dark' ? '切换到浅色主题' : '切换到深色主题'}
  title="切换像素机房配色（夜班靛 / 掌机背光）"
>
  <span class="sw" aria-hidden="true"></span>{theme.current === 'dark' ? '浅色' : '深色'}
</button>

<style>
  .theme-tog {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .theme-tog:hover {
    color: var(--text-hi);
  }
  .theme-tog .sw {
    width: 8px;
    height: 8px;
    background: var(--go);
    border: 2px solid var(--ink);
  }
  /* 装盒款：材质逐字取自 `.btn`（app.css）——台账页页头的另一枚动作钮（指标页「刷新」）
     就是这个材质，同一页不该出现两种按钮长相。不直接套 `.btn` 类：那份样式与本组件
     的配色在两张样式表里比先后，谁赢取决于注入顺序。 */
  .theme-tog.boxed {
    padding: 2px 10px;
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text-2);
    box-shadow: 3px 3px 0 var(--ink);
  }
  .theme-tog.boxed:hover {
    color: var(--text-hi);
    border-color: var(--text-2);
  }
  .theme-tog.boxed:active {
    transform: translate(3px, 3px);
    box-shadow: none;
  }
  @media (max-width: 479px) {
    /* 移动基线：装盒款是这一屏上唯一的动作钮，命中区补到 44px（`.btn` 在这一档同高）。
       只给 `.boxed`——状态行那枚在窄档整条 `display:none`，且 480–748 档的底栏只有
       36px 高，44px 的钮会把它顶破。 */
    .theme-tog.boxed {
      min-height: 44px;
    }
  }
</style>
