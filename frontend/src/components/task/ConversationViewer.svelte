<script lang="ts">
  import type { ChatMessage, ConversationSummary, NodeConversation } from '../../api/types';
  import type { LiveDelta, LiveTool } from '../../realtime/reduce';
  import { formatTokens } from '../../lib/format';
  import MessageBubble from '../render/MessageBubble.svelte';
  import MetadataCard from '../render/MetadataCard.svelte';
  import ToolCallCard from '../render/ToolCallCard.svelte';

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
</script>

<div class="runrow no-scrollbar">
  {#each sorted as c (c.run_id)}
    <button
      type="button"
      class="runchip"
      class:now={selectedRunId === c.run_id}
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
  <div class="empty">选择一个 run 查看会话。</div>
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
      {#each conversation.messages_json as message, i (i)}
        {#if message.role !== 'tool' || message.content}
          <MessageBubble {message} />
        {/if}
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
      <div class="empty">该 run 的会话尚未落库（进行中或已被截断）。</div>
    {/if}
  {/if}
{/if}

<style>
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
    color: var(--text-4);
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
