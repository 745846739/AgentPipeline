<script lang="ts">
  import { onMount } from 'svelte';
  import {
    ApiError,
    clearMarketRepos,
    getMarketRepos,
    installFromRepo,
    listMarketSkills,
    previewSkill,
    saveMarketRepos,
  } from '../api/client';
  import type {
    MarketGroup,
    MarketRepoConfig,
    MarketSkillList,
    MarketSkillRef,
    SkillPreview,
  } from '../api/types';
  import { CompositionGuard, shouldSubmitOnEnter } from '../lib/enterToSend';
  import { addRepo, removeRepo, validateRepo } from '../lib/marketRepos';
  import EmptyState from '../components/ui/EmptyState.svelte';

  /**
   * 设置 · 技能市场（决策 194 换了整层的来源，页骨架照决策 187 不动）。
   *
   * **这一页存在的理由**：远程技能此前只有一条入口——`config.toml`，改完还得重启；界面上
   * 根本没地方改。于是「能装远程技能」这件事对不读配置文件的用户等于不存在。
   *
   * 三块内容，顺序就是使用顺序：
   * 1. **仓名单**——放行哪些 GitHub 仓（保存即生效，不必重启）；校验与 `config.toml`
   *    共用同一个函数（后端 `RepoId` 与这里的 `validateRepo` 同口径），非法项会指明原因；
   * 2. **技能列表**——选中仓里有什么，**按技能目录的父路径分组**（`wshobson/agents` 那种
   *    183 个技能的仓摊平了没法看）。列表钉住浏览那一刻的 commit，顶部写着「基于 <短 SHA>」；
   * 3. **安装 + 立刻看预览**——落盘后马上拉三项预览（推荐去向 / 注入模式与信任态 /
   *    正文特征命中），特征命中要摆在眼前再决定要不要启用。
   *
   * **装 ≠ 启用**：市场只把技能放进技能根；要用它得去「模型与密钥」页的阶段配置里声明
   * （新声明默认只能是 `name` 模式 + 未受信任，决策 181⑤）。
   *
   * 仓名单这份与 `config.toml` 的 `[market] github_repos` 是两级关系（继承决策 22 / 56 /
   * 187 的形状）：保存过就用界面这份，清掉就回到配置文件。页面上把「现在是哪一级」写在
   * 明面上，用户改配置文件却发现「改了没用」时答案就在这儿。
   *
   * **冷启动推荐名单**（后端在 `GET /market/repos` 里给）：它只是若干条**字符串**，是「帮你
   * 起步」的配置默认值，不是审核过的目录。**在名单里点「添加」之前一个字节都不下载**——
   * 这一页因此不会在加载时去 fetch 任何推荐仓（决策 194 的裁决 ④：内置 ≠ 放行）。
   */

  let config = $state<MarketRepoConfig | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  /** 正在编辑的仓名单（保存前是草稿；右边显示的是后端生效值）。 */
  let draft = $state<string[]>([]);
  let newRepo = $state('');
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let saved = $state(false);
  /** 输入法组合态（决策 184）：输入法里敲字再回车是选字，不该直接添加一个仓。 */
  const composingRepo = new CompositionGuard();
  const composingQuery = new CompositionGuard();

  /** 当前查看的仓（从**已保存**的名单里选；草稿里没保存的仓不进列表）。 */
  let selectedRepo = $state<string | null>(null);
  let list = $state<MarketSkillList | null>(null);
  let listing = $state(false);
  let refreshing = $state(false);
  let listError = $state<string | null>(null);
  let query = $state('');

  /** 正在安装的技能目录（`dir` 在仓内唯一，比名字可靠）。 */
  let installing = $state<string | null>(null);
  /** 需要二次确认覆盖的技能目录（报文含「已存在」之后）。 */
  let confirmingOverwrite = $state<string | null>(null);
  let installError = $state<{ message: string; kind?: string; hint?: string } | null>(null);
  /** 刚落盘的技能与它的三项预览（决策 181③：特征命中逐行摆出来）。 */
  let installed = $state<{ name: string; preview: SkillPreview } | null>(null);

  /**
   * 八类失败各自的用户动作（票 02 的表）。
   *
   * **按 `kind` 分支，不按状态码、更不按报文里的字样**：`repo_not_found` 与 `commit_not_found`
   * 都是 404，只有 `kind` 分得开，而它们要用户做的事完全不同（改仓名 / 换 commit）。把八类都
   * 渲染成「装不上」等于把分类白做——这条判定不能有第二个版本（决策 187 的原话）。
   */
  const FAILURE_ACTIONS: Record<string, string> = {
    market_network: '下游不可达：先重试一次；仍不通就查本机网络或代理。',
    repo_not_found: '核对仓名（owner/repo 的拼写），确认后改上面的仓名单，或换一个仓。',
    commit_not_found: '这个 commit 在那个仓里取不到了：点上方的「刷新」取它现在的 tip，再装一次。',
    skill_not_found: '这个仓里没有这个技能目录了：刷新列表后另选一个技能。',
    repo_unreadable: '读不到这个仓——本版不支持私有仓，确认它是公开仓，或换一个仓。',
    digest_mismatch:
      '对象哈希与它声明的 commit 对不上，先别装：把这条报出来（这是该仓内容可疑的证据）。',
    repo_not_allowed: '这个仓还没放行：去上面的仓名单把它加进去并保存，再回来装。',
    download_too_large: '仓太大（上限 64 MiB）：换一个更小的仓，或改指仓里更小的技能目录。',
  };

  async function load() {
    loading = true;
    try {
      config = await getMarketRepos();
      draft = [...config.repos];
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

  const addError = $derived(newRepo.trim() ? validateRepo(newRepo) : null);
  const dirty = $derived(config !== null && draft.join('|') !== config.repos.join('|'));

  /**
   * 关键词过滤：**只对已取下来的这一份做本地过滤**（名字或描述命中，空 = 全部）。
   *
   * 不打 GitHub 的搜索接口（票 05 的裁定：不引 API 面，且那个接口配额 10 次/小时），跨仓
   * 搜索因此只覆盖已拉下来的仓。每次敲键都重新请求还会把「列表钉住 commit」冲掉。
   */
  const groups = $derived.by((): MarketGroup[] => {
    if (!list) return [];
    const needle = query.trim().toLowerCase();
    if (needle === '') return list.groups;
    return list.groups
      .map((g) => ({
        path: g.path,
        skills: g.skills.filter(
          (s) =>
            s.name.toLowerCase().includes(needle) ||
            (s.description ?? '').toLowerCase().includes(needle),
        ),
      }))
      .filter((g) => g.skills.length > 0);
  });

  function tryAdd(raw: string): void {
    const next = addRepo(draft, raw);
    if (!next) {
      saveError = validateRepo(raw) ?? '这个仓已经在名单里了。';
      return;
    }
    draft = next;
    newRepo = '';
    saveError = null;
  }

  function add(): void {
    tryAdd(newRepo);
  }

  /** 从推荐名单里加一条进草稿。**只改草稿，不发任何请求**（决策 194：内置 ≠ 放行）。 */
  function addRecommended(repo: string): void {
    tryAdd(repo);
  }

  function drop(repo: string): void {
    draft = removeRepo(draft, repo);
    // 被移掉的那一行若正在查看，列表也得跟着走：屏幕上不能留着一个已经不在草稿里的仓的技能
    if (selectedRepo === repo) {
      selectedRepo = null;
      list = null;
      listError = null;
      installError = null;
    }
  }

  /** 保存/退回之后把选择收敛到新的生效名单上（未放行的仓不进列表）。 */
  function applyConfig(next: MarketRepoConfig): void {
    config = next;
    draft = [...next.repos];
    if (selectedRepo !== null && !next.repos.includes(selectedRepo)) {
      selectedRepo = null;
      list = null;
    }
  }

  async function save() {
    saving = true;
    saveError = null;
    saved = false;
    try {
      applyConfig(await saveMarketRepos(draft));
      saved = true;
    } catch (err) {
      saveError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  /** 清掉界面那份，回到 `config.toml` 的 `[market] github_repos`（决策 187 / 194）。 */
  async function revert() {
    saving = true;
    saveError = null;
    saved = false;
    try {
      applyConfig(await clearMarketRepos());
    } catch (err) {
      saveError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  /**
   * 列出某个仓的技能。`refresh` → 重新取 tip。
   *
   * 刷新时保留旧列表（只把按钮转起来），换仓时整块换掉——旧列表属于另一个仓，留在屏幕上
   * 就是让人照着错的仓点安装。
   */
  async function viewRepo(repo: string, refresh = false) {
    if (!refresh) {
      selectedRepo = repo;
      list = null;
      query = '';
    }
    listing = !refresh;
    refreshing = refresh;
    listError = null;
    installError = null;
    confirmingOverwrite = null;
    try {
      // q 一律留空：关键词是本地过滤（见上面的 groups），传下去只会多一次无谓的往返
      list = await listMarketSkills(repo, '', refresh);
    } catch (err) {
      if (!refresh) list = null;
      listError = (err as Error).message;
    } finally {
      listing = false;
      refreshing = false;
    }
  }

  function localTime(iso: string): string {
    const t = new Date(iso);
    return Number.isNaN(t.getTime()) ? iso : t.toLocaleString();
  }

  /**
   * 装一个技能。**`list.commit` 一路透传给后端**——用户看到的是某一份，装到的就必须是
   * 那一份（决策 194 裁决 ⑤）。这里绝不在中途「取最新」。
   */
  async function install(ref: MarketSkillRef, overwrite = false) {
    if (!list || selectedRepo === null) return;
    const [owner, repo] = selectedRepo.split('/');
    if (!owner || !repo) return;
    installing = ref.dir;
    installError = null;
    confirmingOverwrite = null;
    try {
      await installFromRepo({
        owner,
        repo,
        commit: list.commit,
        subpath: ref.dir,
        overwrite,
      });
      // 落盘之后立刻取预览：特征命中是「要不要启用」的依据，不能等用户自己去找（决策 181）
      installed = { name: ref.name, preview: await previewSkill(ref.name) };
    } catch (err) {
      const message = (err as Error).message;
      const kind = err instanceof ApiError ? err.kind : undefined;
      // 同名已存在：给一次显式覆盖的机会（与票 09 同口径），不静默覆盖。
      // **按 409 判，不按报文里的字样**：八类市场失败要求"不按 `message` 里的字样分支"
      // 是因为 404 / 400 上各挤着好几类；而 409 在这个端点上**只有一个含义**（同名未确认），
      // 它是状态码里没有歧义的那一种——比拿引擎的措辞当判据稳（那句中文随时可以改）。
      if (err instanceof ApiError && err.status === 409) confirmingOverwrite = ref.dir;
      installError = { message, kind, hint: kind ? FAILURE_ACTIONS[kind] : undefined };
    } finally {
      installing = null;
    }
  }

  function originLabel(c: MarketRepoConfig): string {
    return c.origin === 'settings' ? '界面上的这一份' : 'config.toml 的 [market]';
  }
</script>

<main class="page">
  <a class="crumb" href="#/">← 看板</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 技能市场</h1>
  </div>

  <p class="hintline">
    技能的远程来源是 <b>GitHub 仓</b>（另有本地导入）。<b>放行一个仓 = 允许从它下载引导 agent
    的正文</b>，故这里是一份仓名单，判定按 <span class="mono">owner/repo</span>——GitHub 模式下
    主机恒为 <span class="mono">github.com</span>，按主机放行等于放行任何作者的任何仓。保存<b>当场生效</b>，
    不必重启；清掉界面这一份就回到 <span class="mono">config.toml</span> 的
    <span class="mono">[market] github_repos</span>。
  </p>
  <p class="hintline">
    技能列表钉住<b>浏览那一刻的 commit</b>（顶部写着「基于 &lt;短 SHA&gt;」）：装的与看到的是同一份，
    要跟进更新的版本得显式点「刷新」。本版<b>不支持私有仓</b>，也不放凭据入口。
  </p>

  {#if error}
    <!-- 读不到要有出路（票 02 / R2-07c）：`load()` 只在 onMount 调，没有这颗钮就只能整页刷新。 -->
    <div class="banner error" role="alert">{error}</div>
    <div class="retry">
      <button type="button" class="btn" disabled={loading} onclick={() => void load()}>重试</button>
    </div>
  {:else if loading}
    <div class="banner">正在读取市场配置…</div>
  {:else if config}
    <section class="panel blk">
      <div class="chart-head">
        <h2>仓名单</h2>
        <span class="tag">{originLabel(config)}</span>
      </div>
      <p class="sub">
        现在放行 {config.repos.length} 个仓。把 <span class="mono">owner/repo</span> 加进来 = 信任这个仓的
        技能正文；粘 GitHub 网址也行（<span class="mono">https://github.com/</span> 前缀与
        <span class="mono">.git</span> 后缀会被去掉）。清空并保存 = 不让任何仓进来（本地导入不受影响）。
      </p>

      {#if draft.length === 0}
        <!-- 空态（票 13）：状态 → 下一步 → 可选入口，形状来自 `<EmptyState>`；
             容器沿用本页的虚线空盒 `.blank`（与「仓名单 / 技能列表」两块的分界一致）。 -->
        <div class="blank">
          <EmptyState
            state="仓名单是空的。"
            next="填一个 owner/repo 加进来并保存，就能看到它里面的技能。"
          />
        </div>
      {:else}
        <ul class="src-list">
          {#each draft as r (r)}
            <li class="src-row">
              <span class="mono grow">{r}</span>
              {#if config.repos.includes(r)}
                {#if selectedRepo === r}
                  <span class="tag">正在查看</span>
                {:else}
                  <button type="button" class="btn quiet" onclick={() => void viewRepo(r)}>
                    查看技能
                  </button>
                {/if}
              {:else}
                <span class="sub">未保存</span>
              {/if}
              <button type="button" class="btn quiet" onclick={() => drop(r)}>移除</button>
            </li>
          {/each}
        </ul>
      {/if}

      <div class="subform">
        <input
          class="input mono"
          bind:value={newRepo}
          placeholder="owner/repo"
          aria-invalid={addError !== null ? 'true' : undefined}
          aria-describedby={addError !== null ? 'market-add-error' : undefined}
          onkeydown={(e) => {
            if (!shouldSubmitOnEnter(e, composingRepo.active())) return;
            e.preventDefault();
            add();
          }}
          oncompositionstart={() => composingRepo.start()}
          oncompositionend={() => composingRepo.end()}
        />
        <button
          type="button"
          class="btn"
          disabled={!newRepo.trim() || addError !== null}
          onclick={add}
        >
          ＋ 添加
        </button>
      </div>
      {#if addError}<div class="err" id="market-add-error" role="alert">{addError}</div>{/if}

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
      {#if saveError}<div class="err" role="alert">{saveError}</div>{/if}
      {#if config.origin === 'settings'}
        <p class="sub">
          现在以界面上的这一份为准；<span class="mono">config.toml</span> 里
          <span class="mono">[market] github_repos</span> 的值不再生效——想交还给它就点左边那颗钮。
        </p>
      {/if}

      {#if config.recommended.length > 0}
        <div class="rec">
          <p class="sub">
            冷启动推荐（本机内置的公开技能仓）——它<b>只是帮你起步</b>的配置默认值，不是一份审核过的
            目录：<b>不在下面点「添加」之前，一个字节都不会下载</b>。点「添加」只把它写进上面的草稿，
            要保存之后才生效。
          </p>
          <ul class="rec-list">
            {#each config.recommended as r (r)}
              <li class="rec-row">
                <span class="mono grow">{r}</span>
                {#if draft.includes(r)}
                  <span class="sub">已在草稿里</span>
                {:else}
                  <button type="button" class="btn quiet" onclick={() => addRecommended(r)}>
                    添加
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </section>

    <section class="panel blk">
      <div class="chart-head">
        <h2>技能列表</h2>
        <span class="sub mono">{selectedRepo ?? '（未选择仓）'}</span>
      </div>

      {#if config.repos.length === 0}
        <div class="blank">
          <EmptyState
            state="一个仓都没放行。"
            next="先在上面填一个 owner/repo 并保存，再回来看它里面有什么。"
          />
        </div>
      {:else if selectedRepo === null}
        <div class="blank">
          <EmptyState
            state="还没有选仓。"
            next="点上面仓名单里的「查看技能」，看它里面有什么。"
          />
        </div>
      {:else}
        {#if listing}
          <div class="banner">正在读 {selectedRepo}…</div>
        {/if}
        {#if list}
          <div class="sub listmeta">
            基于 <span class="mono">{list.commit_short}</span>（{localTime(list.listed_at)}）
            <button
              type="button"
              class="btn quiet"
              disabled={refreshing}
              onclick={() => selectedRepo && void viewRepo(selectedRepo, true)}
            >
              {#if refreshing}<span class="spin"></span>{/if}刷新
            </button>
          </div>

          <div class="subform">
            <input
              class="input"
              bind:value={query}
              placeholder="技能名或描述关键词（留空 = 列出全部）"
              onkeydown={(e) => {
                // 过滤是即时的，回车没有动作可提交；护栏照旧接上，免得输入法选字那一次回车
                // 被别的处理者当成一次动作（决策 184）。
                if (!shouldSubmitOnEnter(e, composingQuery.active())) return;
                e.preventDefault();
              }}
              oncompositionstart={() => composingQuery.start()}
              oncompositionend={() => composingQuery.end()}
            />
          </div>
          <p class="sub">
            搜索只过滤<b>这一个仓里已经取下来的技能</b>（不打 GitHub 的搜索接口，那个接口配额
            10 次/小时）——要看别的仓就在上面切换。
          </p>

          {#if groups.length === 0}
            <div class="blank">
              没有命中的技能：这个仓里没有带 <span class="mono">SKILL.md</span> 的目录，或关键词没命中。
              换个词，或点「刷新」取这个仓现在的 tip。
            </div>
          {:else}
            {#each groups as g (g.path)}
              <div class="grp">
                <div class="grp-head mono">{g.path === '' ? '（根）' : g.path}</div>
                <ul class="hit-list">
                  {#each g.skills as s (s.dir)}
                    <li class="hit">
                      <div class="hit-main">
                        <div class="hit-l1">
                          <span class="hit-name mono">{s.name}</span>
                        </div>
                        {#if s.description}<div class="sub">{s.description}</div>{/if}
                        <div class="sub mono hit-dir">{s.dir}</div>
                      </div>
                      <div class="hit-acts">
                        {#if confirmingOverwrite === s.dir}
                          <span class="sub">同名已存在，覆盖？</span>
                          <button
                            type="button"
                            class="btn danger"
                            disabled={installing === s.dir}
                            onclick={() => void install(s, true)}
                          >
                            覆盖安装
                          </button>
                          <button
                            type="button"
                            class="btn quiet"
                            onclick={() => (confirmingOverwrite = null)}
                          >
                            取消
                          </button>
                        {:else}
                          <button
                            type="button"
                            class="btn"
                            disabled={installing === s.dir}
                            onclick={() => void install(s)}
                          >
                            {#if installing === s.dir}<span class="spin"></span>{/if}安装
                          </button>
                        {/if}
                      </div>
                    </li>
                  {/each}
                </ul>
              </div>
            {/each}
          {/if}
        {/if}

        {#if listError}
          <!-- 刷新失败**不再把已列出的技能连控制一起弄没**（票 02 / R2-07a）：
               原来 `listError` 分支排在 `list` 之前，于是 refresh 特意保留的 list 根本渲染不到，
               「刷新」与「查看技能」两颗钮一起消失——唯一出路是换个仓再切回来或整页刷新。
               现在错误与列表并存，且错误自带重试。 -->
          <div class="err" role="alert">{listError}</div>
          <div class="acts">
            <button
              type="button"
              class="btn"
              disabled={listing || refreshing}
              onclick={() => selectedRepo && void viewRepo(selectedRepo, list !== null)}
            >
              重试
            </button>
            {#if list}
              <span class="sub">
                上面这份还是上一次读到的 <span class="mono">{list.commit_short}</span>。
              </span>
            {/if}
          </div>
        {/if}
      {/if}

      {#if installError}
        <div class="err" role="alert">
          {installError.message}
          {#if installError.kind}<span class="tag mono">{installError.kind}</span>{/if}
        </div>
        {#if installError.hint}<div class="sub fail-hint">{installError.hint}</div>{/if}
      {/if}
    </section>

    {#if installed}
      <section class="panel blk">
        <div class="chart-head">
          <h2>刚装上：{installed.name}</h2>
          <span class="tag">尚未启用</span>
        </div>
        <p class="sub">
          技能已落到技能根，<b>还没有任何阶段在用它</b>。要启用请到
          <a href="#/settings/stages">设置 · 阶段配置</a>里声明——新声明默认只能是
          <span class="mono">name</span> 模式 + 未受信任（正文由 <span class="mono">Skill</span>
          工具按需拉取）。
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
</main>

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
  /* 错误横幅下的出路（票 02）：横幅与它的重试钮是同一件事。 */
  .retry {
    margin: 8px 0 12px;
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
  .rec-list,
  .hit-list,
  .prev-list {
    list-style: none;
    margin: 8px 0;
  }
  .src-row,
  .rec-row {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 6px 10px;
    margin-bottom: -2px;
  }
  .rec-row {
    border-style: dashed;
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
  /* 冷启动推荐：与仓名单同页但边界分明（虚线），免得被当成已放行的仓 */
  .rec {
    margin-top: 14px;
    border-top: 2px solid var(--pane);
    padding-top: 10px;
  }
  /* 技能列表：按技能目录的父路径分组（摊平了 183 个技能没法看） */
  .grp {
    margin-top: 12px;
  }
  .grp-head {
    color: var(--text-2);
    letter-spacing: 0.06em;
    margin-bottom: 4px;
    word-break: break-all;
  }
  .listmeta {
    margin-top: 10px;
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
