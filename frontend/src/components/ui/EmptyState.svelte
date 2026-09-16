<script lang="ts">
  /**
   * 空态的唯一形状（UX 审计票 13）：状态 → 下一步 → 可选入口。
   * 凡是提到另一个页面的地方都给 href，点了能到；文字用可读档（--text-3），不用装饰档。
   */
  interface Props {
    /** 状态：现在是空的（说清缺什么）。 */
    state: string;
    /** 下一步做什么（可选，但推荐写）。 */
    next?: string;
    /** 可选入口：点了去哪。 */
    href?: string;
    /** 入口文案，缺省「去看看」。 */
    linkLabel?: string;
  }

  let { state, next, href, linkLabel = '去看看' }: Props = $props();
</script>

<div class="empty">
  <p class="es">{state}</p>
  {#if next}
    <p class="en">{next}</p>
  {/if}
  {#if href}
    <a class="el" href={href}>{linkLabel}</a>
  {/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 10px 4px 14px;
    max-width: 86ch;
  }
  .es,
  .en {
    font-size: 12px;
    line-height: 1.8;
    color: var(--text-3);
  }
  .es {
    color: var(--text-hi);
  }
  .el {
    font-size: 12px;
    color: var(--text-hi);
    border-bottom: 2px solid var(--pane);
    padding-bottom: 2px;
  }
  .el:hover {
    border-bottom-color: var(--text-hi);
  }
</style>
