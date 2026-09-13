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
        <span class="cmd-l1">
          <i class={command.exit_code === null || command.exit_code === 0 ? 'ok' : 'bad'}></i>
          <span class="tm">{formatClock(command.started_at)}</span>
          <span class="src">{command.source === 'system' ? 'sys' : 'agent'}</span>
          <span class="ms">{command.duration_ms !== null ? formatDuration(command.duration_ms) : '—'}</span>
          <span class="ex {command.exit_code && command.exit_code !== 0 ? 'bad' : ''}">
            exit {command.exit_code ?? '—'}
          </span>
        </span>
        <span class="cmd-l2"><span class="c" title={command.command}>{command.command}</span></span>
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
  /* 桌面单表多列：命令 · 时间 · 来源徽标 · 耗时 · 退出码；移动款两行制见下方 */
  .cmd {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
    width: 100%;
    text-align: left;
  }
  .cmd:hover {
    background: var(--hover-bg);
  }
  /* 退出码信号灯：8px 实心像素方块（绿灯 = 成功，红灯 = 非零），不用字符 ✓/✗ */
  .ok,
  .bad {
    flex: none;
    width: 8px;
    height: 8px;
  }
  .ok {
    background: var(--go);
  }
  .bad {
    background: var(--stop);
  }
  .tm {
    color: var(--text-4);
    width: 62px;
    flex: none;
  }
  /* source = agent | system 仍用徽标区分，但共用一表 */
  .src {
    flex: none;
    width: 42px;
    font-size: 12px;
    text-align: center;
    border: 2px solid var(--pane);
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
    width: 64px;
    flex: none;
    text-align: right;
  }
  .ex.bad {
    color: var(--stop);
  }
  /* 输出块：2px 左缘像素条 + 货箱面底，等宽不折行可横滚 */
  .cmdout {
    border-left: 2px solid var(--pane);
    background: var(--panel);
    margin: 4px 0 12px 26px;
    padding: 8px 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-2);
    max-width: 840px;
    white-space: pre-wrap;
    overflow: auto;
    max-height: 360px;
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

  /* 桌面：两行容器不生成盒子，用 order 还原原列序（ok · tm · src · 命令 · ms · ex · 输出） */
  .cmd-l1,
  .cmd-l2 {
    display: contents;
  }
  .cmd .c {
    order: 1;
  }
  .cmd .ms {
    order: 2;
  }
  .cmd .ex {
    order: 3;
  }

  /* ── 移动版（<480px）：命令表两行制（theme-3 §8 转写 4） ── */
  @media (max-width: 479px) {
    .cmds {
      max-width: none;
    }
    .cmd {
      flex-wrap: wrap;
      align-items: center;
      gap: 2px 9px;
      padding: 9px 2px;
      border-bottom: 2px solid var(--hairline);
    }
    .cmd:last-child {
      border-bottom: 0;
    }
    .cmd .cmd-l1 {
      display: flex;
      flex: 1 0 100%;
    }
    .cmd .cmd-l1 .ms,
    .cmd .cmd-l1 .ex {
      order: 0;
    }
    .cmd .cmd-l1 .ms {
      width: auto;
      margin-left: auto;
    }
    .cmd .cmd-l1 .ex {
      width: auto;
    }
    .cmd .cmd-l2 {
      display: flex;
      flex: 1 0 100%;
      margin-top: 3px;
    }
    .cmd .cmd-l2 .c {
      overflow: visible;
      text-overflow: clip;
      white-space: pre-wrap;
      word-break: break-all;
      font-size: 12px;
      color: var(--text);
      line-height: 1.5;
    }
    .cmdout {
      margin: 8px 0 12px 0;
      max-height: none;
      font-size: 12px;
    }
  }
</style>
