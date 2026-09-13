<script lang="ts">
  import { SPRITES, LIGHT_DEVIATIONS, type SpriteName, type SpriteRect } from '../../theme/contract';

  /**
   * 受控像素图元（决策 169 / theme-6-pixel.md §2.3 / §3.2）。
   * 图元只允许来自契约的 sprite 表——新增必须回规格修订，组件不得内联画图。
   * 缺省填 `currentColor`（随宿主状态取色）；契约里写 token 名的 rect（如货箱锁孔、工头脸块）
   * 解析成 `var(--token)`。
   */
  interface Props {
    name: SpriteName;
    /** 显示边长；缺省用契约登记的推荐尺寸（8×8 → 16px，foreman → 48px）。 */
    size?: number;
  }
  let { name, size }: Props = $props();

  const sprite = $derived(SPRITES[name]);
  const px = $derived(size ?? sprite.size);

  function fillOf(rect: SpriteRect): string {
    if (!rect.fill) return 'currentColor';
    if (rect.fill.startsWith('--')) {
      // 工头脸块固定肤色：不用 --text-hi，否则浅色下变墨块（§2.4 偏差②）。
      if (name === 'foreman' && rect.fill === '--text-hi') return LIGHT_DEVIATIONS.foremanFace;
      return `var(${rect.fill})`;
    }
    return rect.fill;
  }
</script>

<svg
  class="sprite"
  viewBox="0 0 {sprite.viewBox} {sprite.viewBox}"
  width={px}
  height={px}
  shape-rendering="crispEdges"
  fill="currentColor"
  aria-hidden="true"
>
  {#each sprite.rects as rect, i (i)}
    <rect x={rect.x} y={rect.y} width={rect.w} height={rect.h} fill={fillOf(rect)} />
  {/each}
</svg>

<style>
  .sprite {
    display: block;
    flex: none;
  }
</style>
