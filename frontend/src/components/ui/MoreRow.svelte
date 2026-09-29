<script lang="ts">
  /**
   * 长列表窗口化的展开行（spec list-windowing 票 01）：「加载更多」的唯一组件壳。
   *
   * 按钮不做滚动自动加载（不与贴底跟随语义打架）；同一颗按钮两套话术由调用点给：
   * 尾部锚定（命令 / 会话）说「已省略前 N 条，点此展开」，头部锚定（看板列）说
   * 「还有 M 张，加载更多」。判据（省略计数怎么算）在 `lib/windowSlice.ts`。
   */
  interface Props {
    label: string;
    onclick: () => void;
  }
  let { label, onclick }: Props = $props();
</script>

<button type="button" class="more" {onclick}>{label}</button>

<style>
  /* 2px 描边、12px 字号是 §3.1 / §5 的全站像素纪律（css-parity 会拦 1px / 11px） */
  .more {
    display: block;
    width: 100%;
    padding: 6px 12px;
    border: 2px solid var(--pane);
    background: none;
    color: var(--text-3);
    font-size: 12px;
    cursor: pointer;
    text-align: center;
  }
  .more:hover {
    color: var(--text);
  }
</style>
