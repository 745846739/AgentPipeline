<script lang="ts">
  import { GEOMETRY, gaugeFilled } from '../../theme/contract';

  /**
   * 分段量表（决策 169 / theme-6-pixel.md §2.3）：全站共用一套 16 段 HP 条质感。
   * 底栏 token 总量、货箱 meta 行共用本组件——不接受任何自定义段色或柔光，
   * 段色只由 `tone` 取四盏信号灯语义（go / warn / stop / dim）。
   */
  type Tone = 'go' | 'warn' | 'stop' | 'dim';
  interface Props {
    /** token 数；由契约的 16 段折算规则点亮（0 不点亮，非零至少 1 段）。 */
    tokens?: number;
    /** 直接给已点亮段数（货箱等已有折算结果的场景）；给了就以它为准。 */
    filled?: number;
    tone?: Tone;
    segments?: number;
  }
  let { tokens = 0, filled, tone = 'go', segments = GEOMETRY.gaugeSegments }: Props = $props();

  const lit = $derived(filled ?? gaugeFilled(tokens));
  const cells = $derived(Array.from({ length: segments }, (_, i) => i));
</script>

<span class="gauge {tone}" aria-hidden="true">
  {#each cells as cell (cell)}<i class={cell < lit ? 'f' : ''}></i>{/each}
</span>

<style>
  .gauge {
    display: inline-flex;
    gap: 2px;
    vertical-align: -1px;
  }
  .gauge i {
    width: 5px;
    height: 10px;
    background: var(--pane);
  }
  /* 点亮段跟随 tone 的 currentColor（与信号灯同色，不引第二套色） */
  .gauge i.f {
    background: currentColor;
  }
  .gauge.go {
    color: var(--go);
  }
  .gauge.warn {
    color: var(--pending);
  }
  .gauge.stop {
    color: var(--stop);
  }
  .gauge.dim {
    color: var(--done);
  }
</style>
