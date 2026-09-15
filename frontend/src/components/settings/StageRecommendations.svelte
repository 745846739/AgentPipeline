<script lang="ts">
  import type { RecommendedStage, SkillPreview } from '../../api/types';
  import { stageKeyLabel } from '../../lib/stageConfigs';

  /**
   * 阶段推荐与一键安装（决策 172①，票 16）。
   *
   * 推荐清单的投递载体是**界面**：清单本身是二进制里的常量（经 `GET /skills/recommendations`
   * 下发），技能正文一律由用户自己安装。一键安装把「落到技能根 + 写进该阶段配置」合成一步，
   * 并把票 11 的三项预览原样摆在按钮下面——**不绕过信任确认**：新装技能一律是名字态 + 未信任。
   */
  interface Props {
    stages: RecommendedStage[];
    busy: string | null;
    /**
     * 最近一次一键安装带回来的三项预览（票 11）。由父组件持有并回填——
     * 装完立刻把正文特征摆给用户看，是「预览不被绕过」在界面上的落点。
     */
    preview: SkillPreview | null;
    /** 一键安装；失败原因（技能不存在 / 摘要不符 / 来源未放行…）由父组件回填到 error。 */
    oninstall: (stage: string, name: string) => void;
  }
  let { stages, busy, preview, oninstall }: Props = $props();

  const risky = $derived(preview?.features.hits ?? []);
</script>

<div class="rec panel">
  <div class="rec-head cond">推荐技能</div>
  <p class="rec-hint">
    按阶段推荐的知识型技能。清单来自内置常量，技能正文需要你自己安装——装进来的技能默认
    <strong>未受信任</strong>，只能以「仅注入名字」进入配置，确认信任后才可切全文。
  </p>

  {#if stages.length === 0}
    <p class="rec-hint">没有可用的推荐清单。</p>
  {:else}
    {#each stages as group (group.stage)}
      <div class="group">
        <div class="stage mono">{stageKeyLabel(group.stage)}</div>
        <ul class="items">
          {#each group.skills as skill (skill.name)}
            <li class="item">
              <span class="name mono">{skill.name}</span>
              <span class="reason">{skill.reason}</span>
              <span class="state" class:on={skill.installed}>
                {skill.installed ? '已安装' : '未安装'}
              </span>
              {#if skill.declared_in.length > 0}
                <span class="used">已启用：{skill.declared_in.join('、')}</span>
              {/if}
              {#if !skill.installed}
                <button
                  type="button"
                  class="btn"
                  disabled={busy !== null}
                  onclick={() => oninstall(group.stage, skill.name)}
                >
                  {#if busy === `${group.stage}:${skill.name}`}<span class="spin"></span>{/if}
                  安装
                </button>
              {/if}
            </li>
          {/each}
        </ul>
      </div>
    {/each}
  {/if}

  {#if risky.length > 0}
    <div class="features">
      <div class="feat-head">
        {preview?.name} 的正文特征（仅供你判断，不构成安装准入）
      </div>
      <ul>
        {#each risky as hit (`${hit.kind}-${hit.line}`)}
          <li>
            <span class="kind mono">{hit.label}</span>
            <span class="line">第 {hit.line} 行</span>
            <span class="text mono">{hit.text}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

<style>
  .rec {
    padding: 14px 16px;
    margin-bottom: 14px;
  }
  .rec-head {
    color: var(--text-hi);
    letter-spacing: 0.08em;
    margin-bottom: 6px;
  }
  .rec-hint {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.6;
    margin-bottom: 10px;
  }
  .group {
    margin-bottom: 10px;
  }
  .stage {
    color: var(--text-2);
    font-size: 12px;
    margin-bottom: 4px;
  }
  .items {
    list-style: none;
    display: grid;
    gap: 4px;
  }
  .item {
    display: grid;
    grid-template-columns: minmax(110px, auto) 1fr auto auto;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    line-height: 1.6;
  }
  .name {
    color: var(--text-hi);
  }
  .reason,
  .used {
    color: var(--text-3);
  }
  .used {
    grid-column: 1 / -1;
  }
  .state {
    color: var(--pending);
  }
  .state.on {
    color: var(--done);
  }
  .features {
    margin-top: 10px;
    border-top: 2px solid var(--pane);
    padding-top: 8px;
  }
  .feat-head {
    font-size: 12px;
    color: var(--pending);
    margin-bottom: 4px;
  }
  .features ul {
    list-style: none;
    display: grid;
    gap: 2px;
  }
  .features li {
    display: flex;
    gap: 8px;
    font-size: 12px;
    line-height: 1.6;
  }
  .kind {
    color: var(--pending);
  }
  .line,
  .text {
    color: var(--text-3);
  }

  @media (max-width: 479px) {
    /* 台账行折成纵向（§5「台账行由横向左右栏折成纵向」）：名字 + 状态一行、理由一行、
       安装钮另起一行通栏。**必须逐格点名**：只把模板换成两列（`1fr auto`）而留着四个
       孩子时，`auto` 列会被理由的 max-content 撑大，`1fr` 的名字列被压到 min-content
       ——技能名在连字符处断成两行（domain-modeling）、「未安装」也断成「未」+「安装」，
       而理由仍被挤在窄列里。 */
    .item {
      grid-template-columns: 1fr auto;
      row-gap: 2px;
    }
    .name {
      grid-column: 1;
      grid-row: 1;
    }
    /* 行号也要定死：理由跨整行，稀疏自动放置会把只定了列的状态格推到理由**下面**，
       每行多出一行空白（状态与名字同行才是这一格的意图）。 */
    .state {
      grid-column: 2;
      grid-row: 1;
      text-align: right;
    }
    .reason {
      grid-column: 1 / -1;
    }
    .item :global(.btn) {
      grid-column: 1 / -1;
      width: 100%;
    }
  }
</style>
