<script lang="ts">
  import { WORKER_FRAMES } from '../../theme/contract';

  /**
   * 挥锤小人（决策 169 / theme-6-pixel.md §3）：站头与列头共用同一枚图元。
   *
   * 节奏由宿主传（`workerRhythm()` 从契约的 `STATE_STYLES.worker` 取）：
   * run 快挥 0.6s / wait 慢挥 1.8s / idle 站立。双帧用**离散 opacity 翻转**
   * （§2.3 原则 4），不是补间动画。提取成组件是因为列头（看板）与站头（移动详情）
   * 需要完全相同的图元与节奏——各写一份必然漂移。
   */
  interface Props {
    rhythm: 'run' | 'wait' | 'idle';
  }
  let { rhythm }: Props = $props();
</script>

<span class="worker {rhythm}" aria-hidden="true">
  <svg
    class="f1"
    viewBox="0 0 8 8"
    width="16"
    height="16"
    shape-rendering="crispEdges"
    fill="currentColor"
  >
    {#each WORKER_FRAMES.raised as r, i (i)}
      <rect x={r.x} y={r.y} width={r.w} height={r.h} />
    {/each}
  </svg>
  <svg
    class="f2"
    viewBox="0 0 8 8"
    width="16"
    height="16"
    shape-rendering="crispEdges"
    fill="currentColor"
  >
    {#each WORKER_FRAMES.struck as r, i (i)}
      <rect x={r.x} y={r.y} width={r.w} height={r.h} />
    {/each}
  </svg>
</span>

<style>
  .worker {
    position: relative;
    display: inline-block;
    width: 16px;
    height: 16px;
    margin-left: 8px;
    flex: none;
  }
  .worker svg {
    position: absolute;
    inset: 0;
  }
  .worker .f2 {
    opacity: 0;
  }
  .worker.run {
    color: var(--go);
  }
  .worker.wait {
    color: var(--pending);
  }
  .worker.idle {
    color: var(--text-4);
  }
  /* 双帧离散翻转：一半周期各显一帧，无补间 */
  @keyframes wA {
    0%,
    50% {
      opacity: 1;
    }
    50.01%,
    100% {
      opacity: 0;
    }
  }
  @keyframes wB {
    0%,
    50% {
      opacity: 0;
    }
    50.01%,
    100% {
      opacity: 1;
    }
  }
  .worker.run .f1 {
    animation: wA 0.6s steps(2) infinite;
  }
  .worker.run .f2 {
    animation: wB 0.6s steps(2) infinite;
  }
  .worker.wait .f1 {
    animation: wA 1.8s steps(2) infinite;
  }
  .worker.wait .f2 {
    animation: wB 1.8s steps(2) infinite;
  }
  @media (prefers-reduced-motion: reduce) {
    .worker svg {
      animation: none !important;
    }
  }
</style>
