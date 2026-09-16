<script lang="ts">
  import type { SkillSummary } from '../../api/types';
  import {
    addSkillDecl,
    canSwitchToFull,
    removeSkillDecl,
    setSkillMode,
    setSkillTrust,
    type SkillDeclDraft,
  } from '../../lib/stageConfigs';
  import { CompositionGuard, shouldSubmitOnEnter } from '../../lib/enterToSend';

  /**
   * 技能声明列表控件（决策 172④，票 15）。
   *
   * 把「自由文本 skills_json」换成结构化控件：每项可切注入模式、可翻信任态、可移除。
   * 阶段级与节点级共用本组件——两处的字段形态与判定规则完全一样，分两份实现必然漂移。
   */
  interface Props {
    decls: SkillDeclDraft[];
    /** 可用技能目录（用于「添加」的候选与描述提示）。 */
    available: SkillSummary[];
    /** 标题下方的说明（节点级与阶段级的差别只在说明里）。 */
    hint?: string;
    onchange: (decls: SkillDeclDraft[]) => void;
  }
  let { decls, available, hint, onchange }: Props = $props();

  /** 本实例的 datalist id：同一页面挂 4 份（阶段级 + 三个节点级），共用 id 会让每个输入框
   *  都指到文档里的第一个 `<datalist>`，候选列表串台（各分组的候选本就不同）。 */
  const uid = $props.id();
  const catalogId = `skill-candidates-${uid}`;

  let picked = $state('');
  let localError = $state<string | null>(null);
  /** 输入法组合态（决策 184）：中文输入法里敲英文再回车，那个回车是选字不是「添加」。 */
  const composing = new CompositionGuard();

  function describe(name: string): string | null {
    return available.find((s) => s.name === name)?.description ?? null;
  }

  /** 可用技能里的候选（排除已声明的）。 */
  const candidates = $derived(available.filter((s) => !decls.some((d) => d.name === s.name)));

  function apply(result: ReturnType<typeof setSkillMode>) {
    if (result.ok) {
      localError = null;
      onchange(result.decls);
    } else {
      localError = result.error;
    }
  }

  function changeMode(index: number, mode: 'full' | 'name') {
    apply(setSkillMode(decls, index, mode));
  }

  function toggleTrust(index: number) {
    apply(setSkillTrust(decls, index, !decls[index]?.trusted));
  }

  function remove(index: number) {
    localError = null;
    onchange(removeSkillDecl(decls, index));
  }

  /** 目录里点条目 = 直接加入（与输入框 + 添加等价，少一次点击）。 */
  function pick(name: string) {
    localError = null;
    onchange(addSkillDecl(decls, name));
  }

  function add() {
    const next = addSkillDecl(decls, picked);
    if (next === decls) {
      if (picked.trim()) localError = `技能 ${picked.trim()} 已在列表里。`;
      return;
    }
    localError = null;
    picked = '';
    onchange(next);
  }
</script>

<div class="decls">
  <div class="decls-head">
    <span class="cond">已启用技能</span>
    {#if hint}<span class="hint">{hint}</span>{/if}
  </div>

  {#if decls.length === 0}
    <p class="empty">还没有启用技能。从下面的可用技能目录里挑一个。</p>
  {:else}
    <ul class="rows">
      {#each decls as decl, i (decl.name)}
        <li class="row">
          <span class="name mono">{decl.name}</span>

          <label class="mode">
            <span class="sr">注入模式</span>
            <select
              class="input mono"
              value={decl.mode}
              onchange={(e) => changeMode(i, e.currentTarget.value as 'full' | 'name')}
            >
              <option value="full" disabled={!canSwitchToFull(decl)}>注入全文</option>
              <option value="name">仅注入名字</option>
            </select>
          </label>

          <span class="trust" class:on={decl.trusted}>
            {#if decl.trusted}
              <span class="lamp go"></span>已信任
            {:else if decl.bare}
              <span class="lamp dim"></span>旧格式 · 未信任
            {:else}
              <span class="lamp pend"></span>未信任
            {/if}
          </span>

          <button type="button" class="btn quiet" onclick={() => toggleTrust(i)}>
            {decl.trusted ? '撤销信任' : '信任此技能'}
          </button>
          <button type="button" class="btn danger" onclick={() => remove(i)}>移除</button>

          {#if describe(decl.name)}
            <span class="desc">{describe(decl.name)}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if candidates.length > 0}
    <div class="catalog">
      <div class="catalog-head cond">可用技能目录</div>
      <ul class="catalog-list">
        {#each candidates as s (s.name)}
          <li class="catalog-item">
            <button type="button" class="pick mono" onclick={() => pick(s.name)}>＋ {s.name}</button>
            {#if s.disable_model_invocation}
              <span class="tag">手动触发</span>
            {/if}
            {#if s.description}<span class="desc">{s.description}</span>{/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if candidates.length > 0 || picked.trim()}
    <div class="add">
      <input
        class="input mono"
        list={catalogId}
        bind:value={picked}
        placeholder="技能名"
        onkeydown={(e) => {
          if (!shouldSubmitOnEnter(e, composing.active())) return;
          e.preventDefault();
          add();
        }}
        oncompositionstart={() => composing.start()}
        oncompositionend={() => composing.end()}
      />
      <datalist id={catalogId}>
        {#each candidates as s (s.name)}
          <option value={s.name}>{s.description ?? ''}</option>
        {/each}
      </datalist>
      <button type="button" class="btn" onclick={add} disabled={!picked.trim()}>＋ 添加</button>
    </div>
  {/if}

  {#if localError}<div class="error">{localError}</div>{/if}
</div>

<style>
  .decls {
    border: 2px solid var(--pane);
    padding: 10px 12px;
    background: var(--panel);
  }
  .decls-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 6px;
  }
  .decls-head .cond {
    color: var(--text-hi);
    letter-spacing: 0.08em;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
    line-height: 1.6;
  }
  .rows {
    list-style: none;
    display: grid;
    gap: 6px;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(120px, 1fr) 140px auto auto auto;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    border-bottom: 2px solid var(--wash);
  }
  .name {
    color: var(--text-hi);
  }
  .trust {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-3);
  }
  .lamp {
    display: inline-block;
    width: 7px;
    height: 7px;
  }
  .lamp.go {
    background: var(--go);
  }
  /* 票 12 逐处判定：**保留琥珀**——未信任是「要不要信这个技能」的待办，且带空心灯的第二编码
     （决策 195：信号色作标记、且已有第二编码的那些不动）。 */
  .lamp.pend {
    background: var(--pending);
  }
  .lamp.dim {
    background: var(--done);
  }
  .desc {
    grid-column: 1 / -1;
    color: var(--text-3);
    font-size: 12px;
    line-height: 1.6;
  }
  .catalog {
    margin-top: 8px;
  }
  .catalog-head {
    color: var(--text-3);
    margin-bottom: 4px;
  }
  .catalog-list {
    list-style: none;
    display: grid;
    gap: 2px;
  }
  .catalog-item {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 12px;
    line-height: 1.6;
  }
  .catalog-item .desc {
    grid-column: auto;
  }
  .pick {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--text-hi);
    cursor: pointer;
  }
  .pick:hover {
    color: var(--go);
  }
  /* 票 12：「手动触发」是技能的一项属性说明，不是待办——回中性档，琥珀留给要人处理的地方。 */
  .tag {
    border: 2px solid var(--pane);
    padding: 0 4px;
    color: var(--text-3);
    font-size: 12px;
  }
  .add {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
  .add .input {
    flex: 1;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-top: 8px;
    line-height: 1.6;
  }

  @media (max-width: 479px) {
    .row {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
