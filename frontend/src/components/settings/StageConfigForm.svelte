<script lang="ts">
  import { untrack } from 'svelte';
  import type { Provider, SkillSummary, StageConfig } from '../../api/types';
  import {
    FOREMAN_STAGE_KEY,
    SKILL_NODES,
    STAGE_KEYS,
    draftFromStageConfig,
    emptyStageConfigDraft,
    nodeSkillsFromJson,
    stageKeyLabel,
    stageMayUseAsk,
    withNodeSkills,
    type SkillDeclDraft,
    type StageConfigDraft,
  } from '../../lib/stageConfigs';
  import SkillDeclList from './SkillDeclList.svelte';

  /**
   * 三档的界面说法（决策 206）。**文案不出现档位名以外的术语**：这一栏要回答的是
   * 「这个阶段的 agent 能不能自己动手」，故每一档的 hint 说的都是**会发生什么**。
   */
  const ENV_MODE_OPTIONS = [
    {
      value: 'auto' as const,
      label: 'auto · 直接执行',
      hint: '文件与命令立即执行（与今天的流水线一致）',
    },
    {
      value: 'ask' as const,
      label: 'ask · 等人按键',
      hint: '生成一条提议，等值班经理在界面上按下确认钮',
    },
    {
      value: 'deny' as const,
      label: 'deny · 拒绝且不告知',
      hint: '连工具都不给它——模型看不到这个选项，硬发也会被拒',
    },
  ];

  /**
   * stage_configs 编辑表单（决策 22 / 46 / 66 / 111 / 129）。
   * 保存是**整条替换**（PUT）：留空的字段会被清空为默认，不是保持原值——UI 明确标注。
   * 父组件用 {#key} 重挂载，故这里用 props 初始化一次 $state 即可。
   */
  interface Props {
    config: StageConfig | null;
    providers: Provider[];
    /** 可用技能目录（`GET /skills`），技能控件的候选来源。 */
    skills: SkillSummary[];
    submitting: boolean;
    error: string | null;
    onsubmit: (draft: StageConfigDraft) => void;
    oncancel: () => void;
  }
  let { config, providers, skills, submitting, error, onsubmit, oncancel }: Props = $props();

  const isNew = untrack(() => config === null);
  let draft = $state<StageConfigDraft>(
    untrack(() => (config ? draftFromStageConfig(config) : emptyStageConfigDraft())),
  );
  let localError = $state<string | null>(null);
  const shownError = $derived(error ?? localError);

  function submit(e: SubmitEvent) {
    e.preventDefault();
    localError = null;
    onsubmit(draft);
  }

  /**
   * 节点级技能写回 `node_overrides_json` 文本（票 15）。
   *
   * 结构化控件与那块自由文本框**共用同一份真相**（那段 JSON 文本）：控件改完立刻回写，
   * 于是两处不会各说一套。其它键（`idle_timeout_sec` 之类）逐字保留。
   */
  function setNodeSkills(node: string, decls: SkillDeclDraft[]) {
    const result = withNodeSkills(draft.node_overrides_json, node, decls);
    if (!result.ok) {
      localError = result.error;
      return;
    }
    localError = null;
    draft.node_overrides_json = result.text;
  }
</script>

<form class="sc-form panel" onsubmit={submit}>
  <div class="form-head cond">
    {isNew ? '新增阶段配置' : `编辑阶段配置 · ${config?.stage}`}
  </div>

  <label class="field">
    <span>阶段 / 伪阶段键</span>
    {#if isNew}
      <select class="input mono" bind:value={draft.stage}>
        {#each STAGE_KEYS as key (key)}
          <option value={key}>{stageKeyLabel(key)}</option>
        {/each}
      </select>
    {:else}
      <input class="input mono" value={config?.stage ?? ''} readonly />
    {/if}
  </label>

  <p class="replace-note">
    保存为<strong>整条替换</strong>：留空的字段会被清空为默认值（不是保持原值）。要保留某项，请确认它已填写。
  </p>

  <div class="grid">
    <label class="field wide">
      <span>provider_id（留空 = 使用系统默认；填写须存在、enabled 且厂商受支持）</span>
      <input
        class="input mono"
        list="stage-provider-ids"
        bind:value={draft.provider_id}
        placeholder="provider ID"
      />
      <datalist id="stage-provider-ids">
        {#each providers as p (p.id)}
          <option value={p.id}>{p.vendor} · {p.model}</option>
        {/each}
      </datalist>
    </label>

    <!-- 环境层档位（决策 206）：一组三档的单选，不是自由文本——它是这套配置里唯一
         直接决定「模型能不能碰这台机器」的字段，一个拼错的值不该有地方写进去。
         留空 = 不配置（真实阶段回落全局默认 auto、值班长回落 ask）。 -->
    <div class="field wide">
      <span>环境层权限档位（文件、命令、技能拉取、子代理）</span>
      <div class="modes" role="radiogroup" aria-label="环境层权限档位">
        <!-- `ask` 只对值班长有意义（规格 §4）：别的阶段无人按那颗钮，配上去等于静默收掉
             这个阶段的环境写动作。这里不摆出来，后端那道门也拒（同一个判据，两处各写一份
             必然漂移——两边的名字与理由都写在 `lib/stageConfigs.ts` 的 `stageMayUseAsk` 上）。 -->
        {#each ENV_MODE_OPTIONS.filter((o) => o.value !== 'ask' || stageMayUseAsk(draft.stage)) as opt (opt.value)}
          <label class="mode">
            <input
              type="radio"
              name="env-mode-{draft.stage}"
              value={opt.value}
              checked={draft.env_mode === opt.value}
              onchange={() => (draft.env_mode = opt.value)}
            />
            <span class="mode-label">{opt.label}</span>
            <span class="mode-hint">{opt.hint}</span>
          </label>
        {/each}
        <label class="mode">
          <input
            type="radio"
            name="env-mode-{draft.stage}"
            value=""
            checked={draft.env_mode === ''}
            onchange={() => (draft.env_mode = '')}
          />
          <span class="mode-label">不配置</span>
          <span class="mode-hint">用这个阶段的缺省（值班长 ask，其余 auto）</span>
        </label>
      </div>
      <p class="mode-note">
        只管环境层。本服务的写接口（建任务、拍板、合入、改配置……）<strong>恒为提议 + 确认钮</strong>，
        不受这一档影响——那类动作会改变流水线的事实。
      </p>
    </div>

    <label class="field">
      <span>temperature</span>
      <input class="input mono" type="number" step="0.1" bind:value={draft.temperature} />
    </label>

    <label class="field">
      <span>max_tokens</span>
      <input class="input mono" type="number" min="1" bind:value={draft.max_tokens} />
    </label>

    <label class="field wide">
      <span>persona_path（相对 / 绝对路径，须可读且非空）</span>
      <input
        class="input mono"
        bind:value={draft.persona_path}
        placeholder="prompts/develop/execute.md"
      />
    </label>

    <label class="field wide">
      <span>persona_append（追加指令，可选）</span>
      <textarea class="input" rows="2" bind:value={draft.persona_append}></textarea>
    </label>

    <label class="field">
      <span>tools_json</span>
      <textarea
        class="input mono json"
        rows="4"
        bind:value={draft.tools_json}
        placeholder='["read_file", "run_command"]'
      ></textarea>
    </label>

    <div class="field wide">
      <SkillDeclList
        decls={draft.skills}
        available={skills}
        hint="阶段级：本阶段所有节点都会带上（与节点级是并集，只增不减）"
        onchange={(decls) => (draft.skills = decls)}
      />
    </div>

    <div class="field wide nodes">
      <div class="nodes-head cond">节点级技能</div>
      <p class="nodes-hint">
        节点级独立于阶段级：合起来是并集（只增不减）。写作 <code>node_overrides_json[node].skills</code>。
      </p>
      {#each SKILL_NODES as node (node)}
        {@const parsed = nodeSkillsFromJson(draft.node_overrides_json, node)}
        <div class="node">
          <SkillDeclList
            decls={parsed.decls}
            available={skills}
            hint={node}
            onchange={(decls) => setNodeSkills(node, decls)}
          />
          {#if parsed.error}
            <!-- 每个节点各报各的：只报第一个节点的错会让另两个节点的坏值静默消失 -->
            <p class="nodes-warn">{node}：{parsed.error}</p>
          {/if}
        </div>
      {/each}
    </div>

    <label class="field">
      <span>idle_timeout_sec</span>
      <input class="input mono" type="number" min="0" bind:value={draft.idle_timeout_sec} />
    </label>

    <label class="field">
      <span>max_duration_sec</span>
      <input class="input mono" type="number" min="0" bind:value={draft.max_duration_sec} />
    </label>

    <!-- 轮数上限（决策 233① / 239）：只对值班长那一行有意义，故只在 foreman 上摆出来
         ——14 行里 13 行都看不见这个格子，比「一个对多数行都无意义的旋钮」安静。
         `min=1`：没有「无上限」这一档（填 0 会被后端拒，这里先挡一道）。 -->
    {#if draft.stage === FOREMAN_STAGE_KEY}
      <label class="field">
        <span>max_rounds</span>
        <input class="input mono" type="number" min="1" bind:value={draft.max_rounds} />
      </label>
    {/if}

    <label class="field wide">
      <span>node_overrides_json</span>
      <textarea
        class="input mono json"
        rows="4"
        bind:value={draft.node_overrides_json}
        placeholder='&#123;"execute": &#123;"idle_timeout_sec": 600&#125;&#125;'
      ></textarea>
    </label>
  </div>

  {#if shownError}<div class="error">{shownError}</div>{/if}

  <div class="actions">
    <button type="button" class="btn quiet" disabled={submitting} onclick={oncancel}>取消</button>
    <button type="submit" class="btn solid" disabled={submitting}>
      {#if submitting}<span class="spin"></span>{/if}
      {isNew ? '创建' : '整条替换'}
    </button>
  </div>
</form>

<style>
  .sc-form {
    padding: 14px 16px;
    margin-bottom: 14px;
  }
  .form-head {
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-hi);
    margin-bottom: 10px;
  }
  /* 票 12 逐处判定：**保留琥珀**——这一块是「保存会清空留空字段」的后果警告，
     读不到就会按错的方式保存（有东西要你处理），正是琥珀该在的地方。 */
  .replace-note {
    font-size: 12px;
    color: var(--pending);
    line-height: 1.6;
    margin-bottom: 10px;
    padding: 7px 10px;
    border: 2px solid var(--pending);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px 14px;
  }
  .field {
    display: block;
  }
  .field.wide {
    grid-column: 1 / -1;
  }
  .field > span {
    display: block;
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 4px;
  }
  textarea.json {
    min-height: 76px;
  }
  .input[readonly] {
    opacity: 0.6;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-top: 8px;
    white-space: pre-wrap;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
  .nodes-head {
    color: var(--text-hi);
    letter-spacing: 0.08em;
    margin-bottom: 4px;
  }
  .nodes-hint,
  .nodes-warn {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.6;
    margin-bottom: 6px;
  }
  /* 票 12 逐处判定：**保留琥珀**——节点级 JSON 解析失败，用户得回去改那段文本。 */
  .nodes-warn {
    color: var(--pending);
  }
  .node {
    margin-bottom: 8px;
  }
  /* 三档单选：竖排（每档之间是「会发生什么」的差别，横排会读成一组并列的开关）。
     不用信号色——这一栏是配置，不是告警（全站唯一的响仍在急停一处，决策 203）。 */
  .modes {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .mode {
    display: grid;
    grid-template-columns: auto auto 1fr;
    align-items: baseline;
    gap: 8px;
    padding: 4px 0;
  }
  .mode-label {
    color: var(--text-hi);
    white-space: nowrap;
  }
  .mode-hint {
    font-size: 12px;
    color: var(--text-3);
  }
  .mode-note {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.6;
    margin-top: 4px;
  }

  @media (max-width: 479px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
