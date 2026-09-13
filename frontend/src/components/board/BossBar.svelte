<script lang="ts">
  import { GEOMETRY, bossExhausted, bossFilled } from '../../theme/contract';

  /**
   * boss 战尝试条（决策 169 / theme-6-pixel.md §2.3）：20 段，`已用 / 上限` 折算点亮段数，
   * **最后一次尝试整条转红**（`exhausted`）。它只是放大版量表，不引入第二套数据。
   */
  interface Props {
    /** 已用尝试数（游标 `validate_attempts`）。 */
    used: number;
    /** 上限；缺省用契约镜像的后端缺省（无端点下发，见 contract.ts 说明）。 */
    limit?: number;
    /** 强制置为"最后一次"（后端权威下发 `retry_exhausted` 时）。 */
    exhausted?: boolean;
  }
  let { used, limit = GEOMETRY.retryLimitMirror, exhausted }: Props = $props();

  const filled = $derived(bossFilled(used, limit));
  const red = $derived(exhausted ?? bossExhausted(used, limit));
  const cells = $derived(Array.from({ length: GEOMETRY.bossSegments }, (_, i) => i));
</script>

<div class="bossbar" role="img" aria-label={`阶段尝试 ${used} / ${limit}`}>
  <span class="bl2">尝试 {used}/{limit}</span>
  <span class="segs {red ? 'red' : ''}">
    {#each cells as cell (cell)}<i class={cell < filled ? 'f' : ''}></i>{/each}
  </span>
</div>

<style>
  .bossbar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px 12px 0;
  }
  .bl2 {
    flex: none;
    color: var(--text-hi);
    font-variant-numeric: tabular-nums;
  }
  .segs {
    flex: 1;
    display: flex;
    gap: 2px;
    border: 2px solid var(--pane);
    background: var(--bg);
    padding: 2px;
  }
  .segs i {
    flex: 1;
    height: 10px;
    background: var(--wash);
  }
  .segs i.f {
    background: var(--go);
  }
  /* 最后一次尝试：整条转红（失败红，是"没有下次了"的信号） */
  .segs.red i.f {
    background: var(--stop);
  }
</style>
