<script lang="ts">
  import { onMount } from 'svelte';
  import {
    clearMarketConfig,
    getMarketConfig,
    installFromMarket,
    previewSkill,
    saveMarketConfig,
    searchMarket,
  } from '../api/client';
  import type { MarketConfig, MarketEntry, SkillPreview } from '../api/types';
  import { CompositionGuard, shouldSubmitOnEnter } from '../lib/enterToSend';
  import { addSource, removeSource, validateSource } from '../lib/marketSources';

  /**
   * 设置 · 技能市场（决策 187）。
   *
   * **这一页存在的理由**：技能市场（决策 172⑤ / 177）此前只有一条入口——`config.toml` 的
   * `[market] allowed_sources`，改完还得重启；界面上根本没地方改。于是「能装远程技能」
   * 这件事对不读配置文件的用户等于不存在。
   *
   * 三块内容，顺序就是使用顺序：
   * 1. **来源白名单**——放行哪些 registry（保存即生效，不必重启）；校验与 `config.toml`
   *    共用同一个函数，非法项会指明是哪一项、为什么；
   * 2. **搜索**——查 registry 的候选（未放行来源的条目不进候选，免得点了才报错）；
   * 3. **安装 + 立刻看预览**——落盘后马上拉三项预览（推荐去向 / 注入模式与信任态 /
   *    正文特征命中），特征命中要摆在眼前再决定要不要启用。
   *
   * **装 ≠ 启用**：市场只把技能放进技能根；要用它得去「模型与密钥」页的阶段配置里声明
   * （新声明默认只能是 `name` 模式 + 未受信任，决策 181⑤）。
   *
   * 白名单这份与 `config.toml` 那份是两级关系：保存过就用界面这份，清掉就回到配置文件。
   * 页面上把「现在是哪一级」写在明面上，用户改配置文件却发现「改了没用」时答案就在这儿。
   */

  let config = $state<MarketConfig | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  /** 正在编辑的来源列表（保存前是草稿；右边显示的是后端生效值）。 */
  let draft = $state<string[]>([]);
  let newSource = $state('');
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let saved = $state(false);
  /** 输入法组合态（决策 184）：输入法里敲字再回车是选字，不该直接添加来源。 */
  const composing = new CompositionGuard();

  let query = $state('');
  let searching = $state(false);
  let searchError = $state<string | null>(null);
  let results = $state<MarketEntry[]>([]);
  let searched = $state(false);
  /** 正在安装的技能名。 */
  let installing = $state<string | null>(null);
  /** 需要二次确认覆盖的技能名（409 之后）。 */
  let confirmingOverwrite = $state<string | null>(null);
  let installError = $state<string | null>(null);
  /** 刚落盘的技能与它的三项预览（决策 181③：特征命中逐行摆出来）。 */
  let installed = $state<{ name: string; preview: SkillPreview } | null>(null);

  async function load() {
    loading = true;
    try {
      config = await getMarketConfig();
      draft = [...config.sources];
      error = null;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
  });

  const addError = $derived(newSource.trim() ? validateSource(newSource) : null);
  const dirty = $derived(
    config !== null && draft.join('|') !== config.sources.join('|'),
  );

  function add() {
    const next = addSource(draft, newSource);
    if (!next) {
      saveError = addError ?? '这一项加不进去。';
      return;
    }
    draft = next;
    newSource = '';
    saveError = null;
  }

  async function save() {
    saving = true;
    saveError = null;
    saved = false;
    try {
      config = await saveMarketConfig(draft);
      draft = [...config.sources];
      saved = true;
    } catch (err) {
      saveError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  /** 清掉界面那份，回到 `config.toml` 的 `[market]`（决策 187）。 */
  async function revert() {
    saving = true;
    saveError = null;
    saved = false;
    try {
      config = await clearMarketConfig();
      draft = [...config.sources];
    } catch (err) {
      saveError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  async function search() {
    searching = true;
    searchError = null;
    installError = null;
    try {
      const res = await searchMarket(query.trim());
      results = res.skills;
      searched = true;
      if (config) config = { ...config, sources: res.sources };
    } catch (err) {
      results = [];
      searched = true;
      searchError = (err as Error).message;
    } finally {
      searching = false;
    }
  }

  async function install(name: string, overwrite = false) {
    installing = name;
    installError = null;
    confirmingOverwrite = null;
    try {
      await installFromMarket(name, overwrite);
      // 落盘之后立刻取预览：特征命中是「要不要启用」的依据，不能等用户自己去找（决策 181）
      installed = { name, preview: await previewSkill(name) };
    } catch (err) {
      const message = (err as Error).message;
      // 409 = 同名已存在：给一次显式覆盖的机会（与票 09 同口径），不静默覆盖
      if (/已存在/.test(message)) confirmingOverwrite = name;
      installError = message;
    } finally {
      installing = null;
    }
  }

  function originLabel(c: MarketConfig): string {
    return c.origin === 'settings' ? '界面上的这一份' : 'config.toml 的 [market]';
  }
</script>

<div class="page">
  <a class="crumb" href="#/">← 看板</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 技能市场</h1>
  </div>

  <p class="hintline">
    远程 registry 是技能的来源之一（另有本地导入）。<b>放行一个来源 = 允许从它下载引导 agent
    的正文</b>，故这里是白名单：只接受 origin（<span class="mono">scheme://host[:port]</span>），
    非回环一律要求 https（明文 http 上 sha256 挡不住中间人，决策 177③）。保存<b>当场生效</b>，
    不必重启。
  </p>

  {#if error}
    <div class="banner error">{error}</div>
  {:else if loading}
    <div class="banner">正在读取市场配置…</div>
  {:else if config}
    <section class="panel blk">
      <div class="chart-head">
        <h2>来源白名单</h2>
        <span class="tag">{originLabel(config)}</span>
      </div>
      <p class="sub">
        当前索引地址：<span class="mono">{config.index_source ?? '（无来源，不允许远程安装）'}</span>
        。清空并保存 = 关掉远程安装（本地导入不受影响）。
      </p>

      {#if draft.length === 0}
        <div class="blank">白名单是空的。填一个可信 registry 的 origin 才能搜索与安装。</div>
      {:else}
        <ul class="src-list">
          {#each draft as s (s)}
            <li class="src-row">
              <span class="mono grow">{s}</span>
              <button
                type="button"
                class="btn quiet"
                onclick={() => (draft = removeSource(draft, s))}
              >
                移除
              </button>
            </li>
          {/each}
        </ul>
      {/if}

      <div class="subform">
        <input
          class="input mono"
          bind:value={newSource}
          placeholder="https://skills.example.com"
          onkeydown={(e) => {
            if (!shouldSubmitOnEnter(e, composing.active())) return;
            e.preventDefault();
            add();
          }}
          oncompositionstart={() => composing.start()}
          oncompositionend={() => composing.end()}
        />
        <button type="button" class="btn" disabled={!newSource.trim() || addError !== null} onclick={add}>
          ＋ 添加
        </button>
      </div>
      {#if addError}<div class="err">{addError}</div>{/if}

      <div class="acts">
        <button type="button" class="btn solid" disabled={saving || !dirty} onclick={() => void save()}>
          {#if saving}<span class="spin"></span>{/if}保存（当场生效）
        </button>
        {#if config.origin === 'settings'}
          <button type="button" class="btn quiet" disabled={saving} onclick={() => void revert()}>
            改回 config.toml 里的那份
          </button>
        {/if}
        {#if dirty}<span class="sub">有未保存的改动。</span>{/if}
        {#if saved}<span class="ok">已保存。</span>{/if}
      </div>
      {#if saveError}<div class="err">{saveError}</div>{/if}
      {#if config.origin === 'settings'}
        <p class="sub">
          现在以界面上的这一份为准；<span class="mono">config.toml</span> 里
          <span class="mono">[market] allowed_sources</span> 的值不再生效——想交还给它就点左边那颗钮。
        </p>
      {/if}
    </section>

    <section class="panel blk">
      <div class="chart-head"><h2>搜索 registry</h2></div>
      <div class="subform">
        <input
          class="input"
          bind:value={query}
          placeholder="技能名或描述关键词（留空 = 列出全部）"
          onkeydown={(e) => {
            if (!shouldSubmitOnEnter(e, composing.active())) return;
            e.preventDefault();
            void search();
          }}
          oncompositionstart={() => composing.start()}
          oncompositionend={() => composing.end()}
        />
        <button type="button" class="btn" disabled={searching || !config.client_ready} onclick={() => void search()}>
          {#if searching}<span class="spin"></span>{/if}搜索
        </button>
      </div>
      {#if !config.client_ready}
        <div class="err">没有可用来源：先在上面填一个 origin 并保存。</div>
      {/if}
      {#if searchError}<div class="err">{searchError}</div>{/if}

      {#if searched && results.length === 0 && !searchError}
        <div class="blank">没有候选。换个关键词，或确认来源索引里有这个技能。</div>
      {:else if results.length > 0}
        <ul class="hit-list">
          {#each results as r (r.name)}
            <li class="hit">
              <div class="hit-main">
                <div class="hit-l1">
                  <span class="hit-name mono">{r.name}</span>
                  <span class="tag">v{r.version}</span>
                  <span class="sub mono">{r.source}</span>
                </div>
                {#if r.description}<div class="sub">{r.description}</div>{/if}
                <div class="sub mono">sha256 {r.sha256.slice(0, 16)}…</div>
              </div>
              <div class="hit-acts">
                {#if confirmingOverwrite === r.name}
                  <span class="sub">同名已存在，覆盖？</span>
                  <button
                    type="button"
                    class="btn danger"
                    disabled={installing === r.name}
                    onclick={() => void install(r.name, true)}
                  >
                    覆盖安装
                  </button>
                  <button type="button" class="btn quiet" onclick={() => (confirmingOverwrite = null)}>
                    取消
                  </button>
                {:else}
                  <button
                    type="button"
                    class="btn"
                    disabled={installing === r.name}
                    onclick={() => void install(r.name)}
                  >
                    {#if installing === r.name}<span class="spin"></span>{/if}安装
                  </button>
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
      {#if installError}<div class="err">{installError}</div>{/if}
    </section>

    {#if installed}
      <section class="panel blk">
        <div class="chart-head">
          <h2>刚装上：{installed.name}</h2>
          <span class="tag">尚未启用</span>
        </div>
        <p class="sub">
          技能已落到技能根，<b>还没有任何阶段在用它</b>。要启用请到
          <a href="#/settings/providers">设置 · 模型与密钥</a>的阶段配置里声明——新声明默认只能是
          <span class="mono">name</span> 模式 + 未受信任（正文由 <span class="mono">Skill</span>
          工具按需拉取，决策 181⑤）。
        </p>
        <div class="prev">
          <div class="prev-col">
            <div class="prev-head">① 推荐去向</div>
            {#if installed.preview.recommendations.length === 0}
              <div class="sub">没有针对该技能的推荐阶段——按需自行声明即可。</div>
            {:else}
              <ul class="prev-list">
                {#each installed.preview.recommendations as rec (rec.stage)}
                  <li><span class="mono">{rec.stage}</span>：{rec.reason}</li>
                {/each}
              </ul>
            {/if}
          </div>
          <div class="prev-col">
            <div class="prev-head">② 注入模式与信任态</div>
            {#if installed.preview.declarations.length === 0}
              <div class="sub">
                尚未被任何配置引用：默认 <span class="mono">{installed.preview.defaults.mode}</span> +
                {installed.preview.defaults.trusted ? '已信任' : '未受信任'}。
                {installed.preview.defaults.note ?? ''}
              </div>
            {:else}
              <ul class="prev-list">
                {#each installed.preview.declarations as d (d.declared_in)}
                  <li>
                    <span class="mono">{d.declared_in}</span>：{d.mode} ·
                    {d.trusted ? '已信任' : '未受信任'}{d.bare ? '（裸字符串）' : ''}
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
          <div class="prev-col wide">
            <div class="prev-head">③ 正文特征（只用于告知）</div>
            {#if !installed.preview.body_available}
              <div class="sub">无正文可扫。</div>
            {:else if installed.preview.features.hits.length === 0}
              <div class="sub">未命中已知特征（命令执行 / 网络调用 / 密钥路径字样）。</div>
            {:else}
              <ul class="prev-list">
                {#each installed.preview.features.hits as h (`${h.kind}:${h.line}`)}
                  <li>
                    <span class="tag">{h.label}</span>
                    <span class="mono">L{h.line}</span>
                    <span class="mono hitline">{h.text}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 20px 24px calc(48px + var(--safeb));
  }
  .banner {
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text-2);
    padding: 12px 14px;
    font-size: 12px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .hintline {
    color: var(--text-3);
    line-height: 1.9;
    margin: 10px 0 18px;
    max-width: 78ch;
  }
  .blk {
    margin-bottom: 18px;
  }
  .chart-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 8px;
  }
  /* 字号只取 12 的整数倍（§2 / css-parity.test.ts）：区块标题用 12px + 强调色，
     与既有设置页的区块标题同一档，不新造 18px 这一级。 */
  .chart-head h2 {
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-hi);
  }
  .chart-head .tag {
    margin-left: auto;
  }
  .sub {
    color: var(--text-3);
    line-height: 1.85;
    max-width: 82ch;
  }
  .sub.mono {
    color: var(--text-3);
  }
  .blank {
    border: 2px dashed var(--pane);
    color: var(--text-3);
    padding: 12px 14px;
    margin: 10px 0;
  }
  .err {
    color: var(--stop);
    line-height: 1.85;
    margin-top: 8px;
  }
  .ok {
    color: var(--go);
  }
  .src-list,
  .hit-list,
  .prev-list {
    list-style: none;
    margin: 8px 0;
  }
  .src-row {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 6px 10px;
    margin-bottom: -2px;
  }
  .grow {
    flex: 1;
    min-width: 0;
    word-break: break-all;
    color: var(--text-hi);
  }
  .subform {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 12px;
    flex-wrap: wrap;
  }
  .subform .input {
    flex: 1;
    min-width: 220px;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 12px;
    flex-wrap: wrap;
  }
  .hit {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 8px 10px;
    margin-bottom: -2px;
  }
  .hit-main {
    flex: 1;
    min-width: 0;
  }
  .hit-l1 {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }
  .hit-name {
    color: var(--text-hi);
  }
  .hit-acts {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  /* 三项预览：三栏，窄屏纵排（§5） */
  .prev {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    margin-top: 10px;
  }
  .prev-col {
    flex: 1;
    min-width: 200px;
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 8px 10px;
  }
  .prev-col.wide {
    flex-basis: 100%;
  }
  .prev-head {
    color: var(--text-2);
    letter-spacing: 0.06em;
    margin-bottom: 6px;
  }
  .prev-list li {
    color: var(--text-3);
    line-height: 1.85;
  }
  .hitline {
    color: var(--text-2);
    word-break: break-all;
  }
  a {
    color: var(--go);
  }
</style>
