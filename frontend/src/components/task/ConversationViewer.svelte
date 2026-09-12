<script lang="ts">
  import type { ChatMessage, ConversationSummary, NodeConversation } from '../../api/types';
  import type { LiveDelta, LiveTool } from '../../realtime/reduce';
  import { summarizeArgs, lineCount, truncate, formatTokens } from '../../lib/format';
  import MarkdownView from '../render/MarkdownView.svelte';
  import MetadataCard from '../render/MetadataCard.svelte';

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

{#snippet toolcard(tool: string, argsSummary: string, resultSummary?: string, phase = 'end')}
  <div class="toolcard">
    <span class="fn mono"><b>{tool}</b>({argsSummary})</span>
    {#if resultSummary}<span class="res {phase}">{resultSummary}</span>{/if}
  </div>
{/snippet}

{#snippet messageBlock(message: ChatMessage, streaming: boolean)}
  {#if message.role === 'system'}
    <details class="sys">
      <summary class="sys-sum">
        <span class="who sys">SYSTEM</span>
        <span class="dim">折叠 · {lineCount(message.content)} 行</span>
      </summary>
      <div class="sysbox mono">{message.content ?? ''}</div>
    </details>
  {:else if message.role === 'user'}
    <div class="msg">
      <div class="who">YOU</div>
      <pre class="userbox">{message.content ?? ''}</pre>
    </div>
  {:else if message.role === 'assistant'}
    <div class="msg assistant">
      <div class="who as">AGT</div>
      {#if message.content}
        <MarkdownView source={message.content} />
      {/if}
      {#each message.tool_calls ?? [] as call (call.id)}
        {@render toolcard(call.function.name, summarizeArgs(call.function.arguments))}
      {/each}
      {#if streaming}<p class="streaming"></p>{/if}
    </div>
  {:else}
    <div class="msg">
      <div class="who">TOOL</div>
      {@render toolcard(message.name ?? 'tool', message.tool_call_id ? `call ${message.tool_call_id}` : '', message.content ? truncate(message.content, 60) : undefined)}
    </div>
  {/if}
{/snippet}

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
          {@render messageBlock(message, false)}
        {/if}
      {/each}
      {#if conversation.metadata_json}
        <MetadataCard metadata={conversation.metadata_json} />
      {/if}
    {/if}

    {#each mergedDeltas as message, i (i)}
      {@render messageBlock(message, i === mergedDeltas.length - 1)}
    {/each}
    {#each selectedTools as t, i (i)}
      {@render toolcard(t.tool, t.args_summary, t.phase, t.phase)}
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
  .runchip {
    font-size: 10.5px;
    padding: 2px 8px;
    border: 1px solid var(--pane);
    color: var(--text-3);
  }
  .runchip:hover {
    color: var(--text-2);
  }
  .runchip.now {
    background: var(--panel);
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
    border-bottom: 1px solid var(--hairline);
    margin-bottom: 12px;
  }
  .convhead h3 {
    font-size: 12px;
    color: var(--text-hi);
  }
  .convhead .m {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-3);
  }
  .live {
    color: var(--go);
  }
  .msg {
    margin-bottom: 13px;
  }
  .who {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--text-3);
    margin-bottom: 3px;
  }
  .who::after {
    content: ' ▸';
    color: var(--text-4);
  }
  .who.sys {
    color: var(--text-4);
  }
  .who.as {
    color: var(--text-hi);
  }
  .dim {
    font-weight: 400;
    letter-spacing: 0;
    color: var(--text-4);
  }
  .sys summary {
    cursor: pointer;
    list-style: none;
  }
  .sys-sum {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .sys-sum .who {
    margin-bottom: 0;
  }
  .sys summary::-webkit-details-marker {
    display: none;
  }
  .sysbox {
    border: 1px dashed var(--pane);
    padding: 6px 10px;
    color: var(--text-3);
    font-size: 11.5px;
    cursor: pointer;
    white-space: pre-wrap;
    max-height: 320px;
    overflow: auto;
  }
  .userbox {
    color: var(--text-2);
    white-space: pre-wrap;
    font-size: 12px;
    font-family: var(--font-mono);
  }
  .assistant :global(p),
  .assistant :global(.md) :global(p) {
    color: var(--text);
    margin: 2px 0 8px;
    max-width: 76ch;
    font-size: 12.5px;
  }
  .assistant :global(.md) {
    color: var(--text);
    font-size: 12.5px;
  }
  .toolcard {
    display: flex;
    align-items: center;
    gap: 10px;
    border-left: 2px solid var(--pane);
    padding: 3px 0 3px 12px;
    margin: 5px 0;
    max-width: 680px;
    font-size: 11px;
    color: var(--text-2);
  }
  .toolcard .fn {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .toolcard .fn b {
    color: var(--text-hi);
    font-weight: 500;
  }
  .toolcard .res {
    margin-left: auto;
    font-size: 10.5px;
    color: var(--text-3);
    flex: none;
  }
  .toolcard .res.end {
    color: var(--go);
  }
  .toolcard .res.error {
    color: var(--stop);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
  }

  /* ── 移动版（<480px）：电文流（theme-3 §8，原型 .msg/.sysbox/.userbox/.assistant/.toolcard） ── */
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
      font-size: 11.5px;
      white-space: nowrap;
    }
    .convhead {
      display: block;
    }
    .convhead h3 {
      font-size: 13px;
      font-weight: 600;
    }
    .convhead .m {
      margin-top: 3px;
      font-size: 11.5px;
    }
    .msg {
      margin-bottom: 14px;
    }
    .who {
      font-size: 11px;
      margin-bottom: 4px;
    }
    .sysbox {
      padding: 8px 10px;
      font-size: 12.5px;
      max-height: none;
    }
    .userbox {
      font-size: 12.5px;
      overflow-wrap: break-word;
    }
    .assistant :global(p),
    .assistant :global(.md) :global(p) {
      font-size: 14px;
      max-width: none;
      margin: 2px 0 9px;
    }
    .assistant :global(.md) {
      font-size: 14px;
    }
    .toolcard {
      flex-wrap: wrap;
      gap: 2px 10px;
      padding: 5px 0 5px 10px;
      margin: 6px 0;
      max-width: none;
      font-size: 12.5px;
    }
    .toolcard .fn {
      overflow: visible;
      overflow-wrap: break-word;
      text-overflow: clip;
      white-space: normal;
    }
    .toolcard .res {
      font-size: 12px;
    }
  }
</style>
