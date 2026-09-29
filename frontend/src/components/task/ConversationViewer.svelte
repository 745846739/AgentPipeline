<script lang="ts">
  import type { ChatMessage, ConversationSummary, NodeConversation } from '../../api/types';
  import type { LiveDelta, LiveTool } from '../../realtime/reduce';
  import { formatTokens } from '../../lib/format';
  import { filterRuns } from '../../lib/runFilter';
  import { DEFAULT_PAGE, windowSlice } from '../../lib/windowSlice';
  import MessageBubble from '../render/MessageBubble.svelte';
  import MetadataCard from '../render/MetadataCard.svelte';
  import ToolCallCard from '../render/ToolCallCard.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import MoreRow from '../ui/MoreRow.svelte';

  interface Props {
    conversations: ConversationSummary[];
    selectedRunId: number | null;
    onselect: (runId: number) => void;
    getConversation?: (runId: number) => NodeConversation | undefined;
    loading?: boolean;
    liveDeltas?: LiveDelta[];
    liveTools?: LiveTool[];
    streamTokens?: { prompt: number; completion: number };
  }

  let {
    conversations,
    selectedRunId,
    onselect,
    getConversation,
    loading = false,
    liveDeltas = [],
    liveTools = [],
    streamTokens,
  }: Props = $props();

  const sorted = $derived(
    [...conversations].sort((a, b) => a.run_id - b.run_id),
  );

  const selected = $derived(
    selectedRunId !== null ? (conversations.find((c) => c.run_id === selectedRunId) ?? null) : null,
  );
  const conversation = $derived(
    selectedRunId !== null ? (getConversation?.(selectedRunId) ?? undefined) : undefined,
  );

  /** 把同一 run 的连续同角色增量合并为一个气泡。 */
  const mergedDeltas = $derived.by<ChatMessage[]>(() => {
    if (selectedRunId === null) return [];
    const out: ChatMessage[] = [];
    for (const d of liveDeltas) {
      if (d.run_id !== selectedRunId) continue;
      const last = out[out.length - 1];
      if (last && last.role === d.role && last.name === d.agent_type) {
        last.content = (last.content ?? '') + d.text;
      } else {
        out.push({ role: d.role as ChatMessage['role'], content: d.text, name: d.agent_type });
      }
    }
    return out;
  });

  const selectedTools = $derived(liveTools.filter((t) => t.run_id === selectedRunId));

  /**
   * 长列表窗口化（spec list-windowing 票 03）：run 药丸墙加关键词过滤，选中 run 的
   * 消息列表接切片原语（默认显尾部 50 条——收尾的元数据卡与流式尾巴都在最底下，
   * 窗口化了才看得见「现在」）。过滤态不进 URL，与折叠态同一口径（决策 217 类比）。
   */
  let runQuery = $state('');
  let shownMsg = $state(DEFAULT_PAGE);

  const shownRuns = $derived(filterRuns(sorted, runQuery));

  // 换 run：窗口游标回缺省——省略计数是按选中 run 的名单算的，旧游标只会有害
  $effect(() => {
    selectedRunId;
    shownMsg = DEFAULT_PAGE;
  });

  /**
   * tool 消息中空 content 的行不渲染（既有判据）；键用**原数组下标**——窗口化之后
   * `each` 的局部 i 不再等于消息在会话里的位置，拿它当键会随窗口滑动而错位。
   */
  const displayed = $derived.by(() => {
    if (!conversation) return [];
    return conversation.messages_json
      .map((message, idx) => ({ message, idx }))
      .filter(({ message }) => message.role !== 'tool' || message.content);
  });
  const msgSlice = $derived(windowSlice(displayed, shownMsg, 'tail'));
</script>

{#if sorted.length === 0}
  <!-- 空态的唯一形状（票 13）：状态 → 下一步。 -->
  <EmptyState
    state="这个任务还没有会话记录。"
    next="流水线跑起来后，每个节点的会话都会出现在这里：模型说了什么、调了哪些工具。"
  />
{:else}
  <div class="runfilter">
    <input
      class="rinput"
      type="search"
      placeholder="滤上面的 run 行：阶段 · 节点 · 子代理 · run id"
      aria-label="按阶段、节点、子代理或 run id 过滤 run 行"
      bind:value={runQuery}
    />
  </div>
  {#if shownRuns.length === 0}
    <div class="empty">没有匹配的 run 行。</div>
  {/if}
  <div class="runrow no-scrollbar">
    {#each shownRuns as c (c.run_id)}
      <button
        type="button"
        class="runchip"
        class:now={selectedRunId === c.run_id}
        aria-pressed={selectedRunId === c.run_id}
        onclick={() => onselect(c.run_id)}
      >
        {c.stage} · {c.node}{c.attempt > 1 ? ` · 尝试 ${c.attempt}` : ''}
        {#if c.agent_type !== 'main'}
          <span class="sub">∟ {c.agent_type}</span>
        {/if}
      </button>
    {/each}
  </div>

  {#if selectedRunId === null}
    <EmptyState state="还没选中哪一轮。" next="点上面任一个节点，看它那一轮跟模型说了什么。" />
  {:else}
    <div class="convhead">
      <h3 class="cond">
        {selected ? `${selected.stage} · ${selected.node} · 尝试 ${selected.attempt}` : `run ${selectedRunId}`}
      </h3>
      <span class="m">
        {selected ? formatTokens(selected.prompt_tokens + selected.completion_tokens) : '0'} tok
        {#if streamTokens}<span class="live">· 流式 +{formatTokens(streamTokens.prompt + streamTokens.completion)}</span>{/if}
      </span>
    </div>

    {#if loading && !conversation}
      <div class="empty">正在加载会话…</div>
    {:else}
      {#if conversation}
        {#if msgSlice.omittedBefore > 0}
          <MoreRow
            label={`已省略前 ${msgSlice.omittedBefore} 条，点此展开`}
            onclick={() => (shownMsg = Math.min(shownMsg + DEFAULT_PAGE, displayed.length))}
          />
        {/if}
        {#each msgSlice.visible as row (row.idx)}
          <MessageBubble message={row.message} />
        {/each}
        {#if conversation.metadata_json}
          <MetadataCard metadata={conversation.metadata_json} />
        {/if}
      {/if}

      {#each mergedDeltas as message, i (i)}
        <MessageBubble {message} streaming={i === mergedDeltas.length - 1} />
      {/each}
      {#each selectedTools as t, i (i)}
        <ToolCallCard tool={t.tool} argsSummary={t.args_summary} resultSummary={t.phase} phase={t.phase} />
      {/each}

      {#if !conversation && mergedDeltas.length === 0 && selectedTools.length === 0}
        <EmptyState
          state="这一轮的会话还没落库。"
          next="它可能还在进行中、或者已经被截断——等这一轮跑完再回来看。"
        />
      {/if}
    {/if}
  {/if}
{/if}

<style>
  /* run 行的过滤框（票 03）：2px 描边、12px 字号是全站像素纪律 */
  .runfilter {
    margin-bottom: 8px;
  }
  .rinput {
    width: 100%;
    max-width: 420px;
    padding: 4px 8px;
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text);
    font-size: 12px;
  }
  .runrow {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  /* run 药丸 = 工位标签盒的小号变体：2px 描边，当前 = wash 实底 + 描边上浮 */
  .runchip {
    font-size: 12px;
    padding: 2px 8px;
    border: 2px solid var(--pane);
    color: var(--text-3);
  }
  .runchip:hover {
    color: var(--text);
  }
  .runchip.now {
    background: var(--wash);
    color: var(--text-hi);
    border-color: var(--text-2);
  }
  .sub {
    color: var(--text-3);
  }
  .convhead {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    padding-bottom: 8px;
    border-bottom: 2px solid var(--pane);
    margin-bottom: 12px;
  }
  .convhead h3 {
    font-size: 12px;
    color: var(--text-hi);
  }
  .convhead .m {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-3);
  }
  .live {
    color: var(--go);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
  }

  /* 消息 / 工具卡样式归 render/ 共享件（MessageBubble / ToolCallCard）所有，
     此处不再复刻——frontend-design.md §8「渲染件复用」。 */

  /* ── 移动版（<480px）：运行条横向滚动（§5 移动款） ── */
  @media (max-width: 479px) {
    .runrow {
      flex-wrap: nowrap;
      overflow-x: auto;
      padding-bottom: 2px;
      -webkit-overflow-scrolling: touch;
    }
    .runchip {
      flex: none;
      display: inline-flex;
      align-items: center;
      min-height: 30px;
      padding: 0 9px;
      white-space: nowrap;
    }
    .convhead {
      display: block;
    }
    .convhead h3 {
      font-size: 12px;
    }
    .convhead .m {
      margin-top: 3px;
      font-size: 12px;
    }
  }
</style>
