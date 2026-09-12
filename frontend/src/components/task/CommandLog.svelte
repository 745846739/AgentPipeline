<script lang="ts">
  import type { NodeCommand } from '../../api/types';
  import { formatClock } from '../../lib/format';
  import { formatDuration } from '../../lib/pipeline';

  interface Props {
    commands: NodeCommand[];
    /** 完整输出（已卸载文件 / 已加载）。 */
    outputFor?: (command: NodeCommand) => string | null;
    /** 请求加载完整输出 `GET /commands/{id}/output`。 */
    onload?: (commandId: number) => Promise<void> | void;
    /** SSE 追加的输出（进行中的命令）。 */
    streamedFor?: (command: NodeCommand) => string | null;
  }

  let { commands, outputFor, onload, streamedFor }: Props = $props();

  let expanded = $state<number | null>(null);
  let loading = $state<number | null>(null);

  async function toggle(command: NodeCommand) {
    if (expanded === command.id) {
      expanded = null;
      return;
    }
    expanded = command.id;
    const has = outputFor?.(command) ?? streamedFor?.(command);
    if (!has && onload) {
      loading = command.id;
      try {
        await onload(command.id);
      } finally {
        loading = command.id === loading ? null : loading;
      }
    }
  }

  function outputText(command: NodeCommand): string {
    return (
      outputFor?.(command) ??
      streamedFor?.(command) ??
      command.stdout_preview ??
      (command.stdout_path ? '（正在加载完整输出…）' : '（该命令未卸载完整输出，只有 preview）')
    );
  }
</script>

{#if commands.length === 0}
  <div class="empty">还没有命令记录。</div>
{:else}
  <div class="cmds">
    {#each commands as command (command.id)}
      <button type="button" class="cmd" onclick={() => toggle(command)}>
        <span class={command.exit_code === null || command.exit_code === 0 ? 'ok' : 'bad'}>
          {command.exit_code === null ? '…' : command.exit_code === 0 ? '✓' : '✗'}
        </span>
        <span class="tm">{formatClock(command.started_at)}</span>
        <span class="src">{command.source === 'system' ? 'sys' : 'agent'}</span>
        <span class="c" title={command.command}>{command.command}</span>
        <span class="ms">{command.duration_ms !== null ? formatDuration(command.duration_ms) : '—'}</span>
        <span class="ex {command.exit_code && command.exit_code !== 0 ? 'bad' : ''}">
          exit {command.exit_code ?? '—'}
        </span>
      </button>
      {#if expanded === command.id}
        {#if loading === command.id}
          <div class="cmdout">正在加载完整输出…</div>
        {:else}
          <pre class="cmdout"><span class="ln">$ {command.command}</span>
{outputText(command)}{#if command.exit_code !== null}
<span class="fin">[exit {command.exit_code}]{command.duration_ms !== null ? `  ${formatDuration(command.duration_ms)}` : ''}</span>{/if}</pre>
        {/if}
      {/if}
    {/each}
  </div>
{/if}

<style>
  .cmds {
    max-width: 900px;
  }
  .cmd {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-2);
    cursor: pointer;
    width: 100%;
    text-align: left;
  }
  .cmd:hover {
    background: var(--hover-bg);
  }
  .ok {
    color: var(--go);
    flex: none;
  }
  .bad {
    color: var(--stop);
    flex: none;
  }
  .tm {
    color: var(--text-4);
    width: 62px;
    flex: none;
  }
  .src {
    flex: none;
    width: 36px;
    font-size: 9.5px;
    text-align: center;
    border: 1px solid var(--pane);
    color: var(--text-3);
  }
  .c {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ms {
    color: var(--text-3);
    width: 52px;
    flex: none;
    text-align: right;
  }
  .ex {
    color: var(--text-3);
    width: 44px;
    flex: none;
    text-align: right;
  }
  .cmdout {
    border-left: 2px solid var(--pane);
    background: var(--panel);
    margin: 4px 0 12px 26px;
    padding: 8px 12px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-2);
    max-width: 840px;
    white-space: pre-wrap;
    max-height: 360px;
    overflow: auto;
  }
  .cmdout .ln {
    color: var(--text-3);
  }
  .cmdout .fin {
    color: var(--go);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
  }
</style>
