<script lang="ts">
  import Sprite from '../render/Sprite.svelte';
  import { completion } from '../../stores/completion.svelte';

  /**
   * 任务完成横幅（票 08 / theme-6-pixel.md §3「完成反馈」）。
   *
   * 顶部居中；奖杯 sprite（`SPRITES.trophy`）+「任务完成」+ diff 摘要 +「收下」按钮。
   * **无入场动画**（像素纪律禁止淡入滑入，§3 / §7）——本组件不声明任何
   * `animation` / `transition`，`prefers-reduced-motion` 下行为因此天然一致。
   *
   * 层叠与位置（与既有 toast 并存，票面第 3 条）：
   * - 横幅 `top: 104px`（移动款 148px，§6 定值）居中；toast 仍在右下（`right/bottom:16px`）。
   * - 横幅 `z-index: 35`：高于底栏 30 / 移动动作坞 31，低于对话框 60 与 toast 70。
   *   两者位置不重叠；这个次序让 toast 始终是最高层，横幅不会盖住对话框。
   *
   * 摘要不可得时 `stats === null`：只显示标题与「已合入」，不画 0（票面第 2 条）。
   * 文案沿用原型：▪ 分隔、`−` 用 U+2212 减号。
   */
  const notice = $derived(completion.notice);
</script>

{#if notice}
  <div class="qtoast" role="status" aria-live="polite" aria-label="任务完成">
    <span class="sp" aria-hidden="true"><Sprite name="trophy" size={24} /></span>
    <span class="qt">任务完成</span>
    <span class="sep" aria-hidden="true">▪</span>
    <span class="body">{notice.title}</span>
    {#if notice.stats}
      <span class="sep" aria-hidden="true">▪</span>
      <span class="diff mono num"
        ><span class="a">+{notice.stats.insertions}</span> <span class="d">−{notice.stats.deletions}</span></span
      >
    {/if}
    <span class="sep" aria-hidden="true">▪</span>
    <span class="merged">已合入</span>
    <button type="button" class="btn take" onclick={() => completion.dismiss()}>收下</button>
  </div>
{/if}

<style>
  .qtoast {
    position: fixed;
    top: 104px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 35;
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: calc(100vw - 32px);
    padding: 10px 14px;
    background: var(--panel);
    border: 2px solid var(--go);
    border-radius: 0;
    /* 容器硬投影（§2.3）；横幅无入场动画，投影是静态的 */
    box-shadow: 4px 4px 0 var(--ink);
    font-size: 12px;
  }
  .sp {
    display: inline-flex;
    color: var(--go);
  }
  /* 「任务完成」用琥珀——原型 `.qtoast .qt` 即此；琥珀仍是全站唯一告警色，
     此处不引入第二种警示色，只是同一 token 的强调用法。 */
  .qt {
    color: var(--pending);
    white-space: nowrap;
  }
  .sep {
    color: var(--text-4);
  }
  .body {
    color: var(--text-hi);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a {
    color: var(--diff-add);
  }
  .d {
    color: var(--diff-del);
  }
  .merged {
    color: var(--go);
    white-space: nowrap;
  }
  .take {
    flex: none;
  }

  @media (max-width: 479px) {
    /* 顶栏三行 ≈138px → 横幅 top 148px，否则被顶栏压住（§6 定值） */
    .qtoast {
      top: 148px;
      left: 12px;
      right: 12px;
      max-width: none;
      transform: none;
    }
    .take {
      margin-left: auto;
    }
  }
</style>
