<script lang="ts">
  import type { ChatMessage } from '../../api/types';
  import { summarizeArgs, lineCount, truncate } from '../../lib/format';
  import MarkdownView from './MarkdownView.svelte';
  import ToolCallCard from './ToolCallCard.svelte';

  interface Props {
    message: ChatMessage;
    /** 进行中的 run 最后一条消息显示尾随光标。 */
    streaming?: boolean;
  }
  let { message, streaming = false }: Props = $props();

  const toolCalls = $derived(message.tool_calls ?? []);
</script>

{#if message.role === 'system'}
  <details class="sys">
    <summary class="sys-sum">
      <span class="who sys">SYS</span>
      <span class="dim">折叠 · {lineCount(message.content)} 行</span>
    </summary>
    <div class="sysbox">{message.content ?? ''}</div>
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
    {#each toolCalls as call (call.id)}
      <ToolCallCard tool={call.function.name} argsSummary={summarizeArgs(call.function.arguments)} />
    {/each}
    {#if streaming}<p class="streaming"></p>{/if}
  </div>
{:else}
  <div class="msg">
    <div class="who">TOOL</div>
    <ToolCallCard
      tool={message.name ?? 'tool'}
      argsSummary={message.tool_call_id ? `call ${message.tool_call_id}` : ''}
      resultSummary={message.content ? truncate(message.content, 60) : undefined}
    />
  </div>
{/if}

<style>
  .msg {
    margin-bottom: 14px;
  }
  /* 发言者行：弱灰小字 + 前缀 `▸`（装饰由 CSS 生成，是亮度编码不是状态字形） */
  .who {
    font-family: var(--font-cond);
    font-size: 12px;
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
  .sys-sum {
    display: flex;
    align-items: baseline;
    gap: 8px;
    cursor: pointer;
    list-style: none;
  }
  .sys-sum::-webkit-details-marker {
    display: none;
  }
  .dim {
    letter-spacing: 0;
    color: var(--text-3);
    font-size: 12px;
  }
  /* system 折叠块 = 像素框（与原型 .sysbox 一致） */
  .sysbox {
    border: 2px solid var(--pane);
    background: var(--bg);
    padding: 6px 10px;
    color: var(--text-3);
    font-size: 12px;
    font-family: var(--font-mono);
    white-space: pre-wrap;
    max-height: 320px;
    overflow: auto;
    cursor: pointer;
  }
  .userbox {
    color: var(--text-2);
    white-space: pre-wrap;
    font-family: var(--font-ui);
    font-size: 12px;
    line-height: 1.6;
  }
  /* 助手段落最大 76ch：提高特异性压过 MarkdownView 自身的 .md（80ch），
     不改 MarkdownView 的公共宽度，也不波及同列的 ToolCallCard。 */
  .msg.assistant :global(.md) {
    max-width: 76ch;
  }

  /* 移动版（<480px）：电文流（theme-3 §8） */
  @media (max-width: 479px) {
    .msg {
      margin-bottom: 14px;
    }
    .who {
      font-size: 12px;
      margin-bottom: 4px;
    }
    .sysbox {
      padding: 8px 10px;
      font-size: 12px;
      max-height: none;
    }
    .userbox {
      font-size: 12px;
      overflow-wrap: break-word;
    }
    .msg.assistant :global(.md) {
      max-width: none;
      font-size: 12px;
    }
  }
</style>
