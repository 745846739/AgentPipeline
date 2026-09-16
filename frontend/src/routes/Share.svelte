<script lang="ts">
  import { onMount } from 'svelte';
  import {
    ApiError,
    clearServerLan,
    fetchPairingToken,
    getServerInfo,
    qrSvgUrl,
    resetPairing,
    setServerLan,
  } from '../api/client';
  import type { ServerAddress, ServerInfo } from '../api/types';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import { changeLanMode } from '../lib/lanToggle';
  import { sharePanel } from '../lib/sharePairing';

  /**
   * 局域网分享页（决策 167 / 186 / 189）：手机扫码接入。
   *
   * 页面只做一件事——把「手机能连上的地址」变成一个可扫的二维码。地址由后端
   * 枚举网卡得出（crates/app/src/lan.rs），前端不做任何猜测：多网卡 / VPN 环境下
   * 选错地址的表现是「扫了打不开」，故这里把后端排好序的推荐项放大，其余列为备选。
   *
   * 仅回环绑定时不显示二维码（拷给手机也连不上），改为给出**一颗真的能按的钮**
   * （决策 186）：绑定可以在运行时改，不必再去改环境变量重启。**这一页跑在
   * localhost，所以那颗钮按得动**——后端只允许回环来源改绑（局域网来源 403）。
   * 二维码由**后端渲染** SVG（决策 167），前端不引 QR 库。
   *
   * **取不到配对令牌时也不画码**（决策 189）：那张码里没有令牌，扫了配不上，却与正常的
   * 那张看起来完全一样。这一条把「骗人的码」换成了「说清要去哪台机器上打开」的指引块。
   */

  let info = $state<ServerInfo | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** 当前选中用于生成二维码的地址（缺省取后端推荐的首项）。 */
  let selected = $state<string | null>(null);
  let copied = $state<string | null>(null);
  /**
   * 配对令牌（决策 182㉙ / 189）。**取不到是常态而不是错误**：`GET /pairing/token` 只在回环
   * 可读——手机自己打开这一页、或在电脑上用局域网地址打开这一页，必然读到 403（那正是这条
   * 护栏的意义）。故这里把「没取到」的三种来源分开存：还在读 / 不是从本机打开的 / 别的故障
   * ——它们的下一步动作不同，混成一个 `null` 就只能给一句谁都对不上的话。
   */
  let tokenState = $state<
    | { kind: 'pending' }
    | { kind: 'ok'; token: string }
    | { kind: 'refused' }
    | { kind: 'failed'; message: string }
  >({ kind: 'pending' });
  let reset = $state(false);
  /** 正在改绑（按钮转圈）；改绑的判定以重读为准（`lib/lanToggle.ts`）。 */
  let switching = $state(false);
  /** 改绑之后要说的话：成功也可能是「但启动参数说了算」，失败要带原因。 */
  let switchNote = $state<string | null>(null);
  let switchError = $state<string | null>(null);
  /** 这一页是从哪个 origin 打开的（决策 189）：说清「为什么这里读不到令牌」要用它。 */
  const origin = typeof window !== 'undefined' ? window.location.origin : '';

  const addresses = $derived(info?.addresses ?? []);
  /**
   * 二维码/复制栏里那个地址。**没有令牌就不画码**（决策 189）——判定住在
   * `lib/sharePairing.ts`，那里钉着「裸地址的码扫了也配不上，而它看起来与正常的那张一样」。
   */
  const panel = $derived(
    sharePanel({
      info,
      addresses,
      selected,
      token: tokenState.kind === 'ok' ? tokenState.token : null,
    }),
  );
  /** 有了它才画码（决策 189）：`null` = 这一页拿不到令牌，改画「去哪台机器上打开」的指引。 */
  const qrTarget = $derived(panel.kind === 'paired-qr' ? panel.target : null);

  /**
   * 取一次配对令牌，并**把失败分档**：403 是「这一页不是从本机打开的」（这条护栏本身，
   * 不是故障，报文也不必惊动使用者）；其余才是真故障，原样报出来。
   */
  async function loadToken() {
    try {
      tokenState = { kind: 'ok', token: (await fetchPairingToken()).token };
    } catch (err) {
      tokenState =
        err instanceof ApiError && err.status === 403
          ? { kind: 'refused' }
          : { kind: 'failed', message: (err as Error).message };
    }
  }

  onMount(async () => {
    try {
      info = await getServerInfo();
      selected = addresses[0]?.url ?? null;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
    // 令牌单取：它失败不影响这一页对「手机怎么连上」的回答，故不并进上面那个 try。
    await loadToken();
  });

  /**
   * 按下「绑定全网卡」/「只绑本机」。
   *
   * 判定以**重读到的绑定地址**为准（`lib/lanToggle.ts`）：改绑会切断当前连接，
   * 包括这次请求自己那条——把传输失败当失败会误报，当成功会掩盖真错误。
   */
  async function switchLan(enabled: boolean) {
    switching = true;
    switchError = null;
    switchNote = null;
    try {
      const result = await changeLanMode(enabled, {
        setLan: setServerLan,
        clearLan: clearServerLan,
        info: getServerInfo,
        sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
      });
      if (result.ok) {
        info = result.info;
        switchNote = result.note;
        // 改绑会切断 SSE 与在飞请求，令牌与地址都重取一次：这一页的其余部分依赖它们
        selected = result.info.addresses[0]?.url ?? null;
        await loadToken();
      } else {
        switchError = result.message;
        // 失败也要把真实状态摆回来（它可能已经变了，或本来就是别的）
        try {
          info = await getServerInfo();
        } catch {
          /* 读不到就保留原读数 */
        }
      }
    } finally {
      switching = false;
    }
  }

  async function doReset() {
    reset = true;
    try {
      tokenState = { kind: 'ok', token: (await resetPairing()).token };
    } catch (err) {
      error = (err as Error).message;
    } finally {
      reset = false;
    }
  }

  async function copy(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      copied = url;
      // 2s 后清掉「已复制」态，避免长时间占位误导
      setTimeout(() => {
        if (copied === url) copied = null;
      }, 2000);
    } catch {
      // 非安全上下文（http 且非 localhost）下 clipboard 不可用：
      // 不报错打断，用户仍可手动选中二维码下方的地址文本
      copied = null;
    }
  }

  /** 首选地址排在首位时高亮；用户手动切换后仍按后端 preferred 标记着色。 */
  function isPreferred(a: ServerAddress) {
    return a.preferred;
  }
</script>

<div class="page">
  <a class="crumb" href="#/">← 看板</a>
  <div class="p-head">
    <h1 class="p-title">手机访问</h1>
  </div>

  {#if loading}
    <div class="banner">正在读取服务地址…</div>
  {:else if error}
    <div class="banner error">{error}</div>
  {:else if info}
    {#if info.loopback_only}
      <section class="gate">
        <!-- 空态（票 13）+ 琥珀收敛（票 12）：这一块是「现在什么也拿不到 + 下一步按哪颗钮」，
             不是告警，故标题回到中性亮档；片段形状来自 `<EmptyState>`。 -->
        <EmptyState
          state="手机现在连不上这台机器"
          next="手机和电脑不在同一个地址空间：127.0.0.1（当前绑定 {info.host}）在手机上指向手机自己，扫码必然打不开。要让手机访问，得让服务监听局域网网卡——按下面那颗钮。"
        />
        <!-- 决策 186：这一颗就是「改绑」的入口，不必再去改环境变量重启。 -->
        <div class="switch">
          <button
            type="button"
            class="btn solid"
            disabled={switching}
            onclick={() => void switchLan(true)}
          >
            {switching ? '正在改绑…' : '绑定全网卡（开启手机访问）'}
          </button>
          <span class="switch-note">按下即生效，不必重启；选择会被记住。</span>
        </div>
        {#if switchNote}<p class="note ok">{switchNote}</p>{/if}
        {#if switchError}<p class="note bad">{switchError}</p>{/if}
        <p class="note">
          手机加载的页面与 API <span class="hi">同源</span>，因此无需额外放行 origin
          （跨源防护只拦异源写请求，同源写请求自带客户端头）。
        </p>
        <p class="note">
          改绑<b>只允许从本机</b>发起（局域网来源 403）——否则同网段的任何设备都能把它打开。
          扫了码的手机若改不了，回到这台电脑上按。
        </p>
        <details class="manual">
          <summary>也可以在启动时指定（改完要重启）</summary>
          <p class="how">命令行启动时绑定全网卡：</p>
          <pre><code>agent-pipeline serve --host 0.0.0.0</code></pre>
          <p class="how">或在配置文件里改：</p>
          <pre><code>[server]
host = "0.0.0.0"</code></pre>
          <p class="how">桌面应用启动时带上环境变量：</p>
          <pre><code>AGENTPIPELINE_LAN=1</code></pre>
          <p class="note">
            启动时指定的绑定<b>优先于这里的按钮</b>：那样启动时，按钮只改得动
            这一次，重启后仍按启动参数来。当前这次绑定来自
            <span class="mono">{info.bind_source === 'startup' ? '启动参数' : info.bind_source === 'settings' ? '界面设置' : '配置文件'}</span>。
          </p>
        </details>
        <p class="warn">
          ⚠ 服务能触发真实 LLM 调用并读取全部会话，开放局域网前请确认所在网段可信
          （更稳妥可用 SSH 隧道 / Tailscale）。
        </p>
      </section>
    {:else if addresses.length === 0}
      <section class="gate">
        <!-- 空态（票 13）：网卡枚举没给出可用地址——说清状态与下一步，再给手动办法。 -->
        <EmptyState
          state="没有找到可用的局域网地址"
          next="服务已绑定 {info.host}:{info.port}，但网卡枚举没有返回可访问的 IPv4 地址——可能是终端缺少网络信息权限，或这台机器当前没有连上局域网。"
        />
        <p class="note">
          可手动用本机局域网 IP 访问：<span class="mono">http://&lt;本机 IP&gt;:{info.port}</span>
        </p>
      </section>
    {:else}
      {#if tokenState.kind === 'pending'}
        <div class="banner">正在读取配对令牌…</div>
      {:else if qrTarget}
        <div class="qrbox">
          <!-- 二维码底盒恒白：扫描器依赖明暗对比，浅色主题也不例外（§3.1） -->
          <div class="qr-qr">
            <img
              src={qrSvgUrl(qrTarget)}
              alt="扫码访问 {qrTarget}"
              width="240"
              height="240"
            />
          </div>
          <div class="qr-side">
            <div class="chart-head">
              <span class="reg-name">扫码在手机上打开</span>
              <span class="port mono">:{info.port}</span>
            </div>
            <div class="picked mono">{qrTarget}</div>
            <button
              type="button"
              class="btn"
              onclick={() => copy(qrTarget)}
            >
              {copied === qrTarget ? '已复制' : '复制地址'}
            </button>
            <p class="qr-cap">
              手机需与电脑在同一局域网（同一 Wi-Fi）。扫码后可直接使用看板与任务详情，
              实时进度经 SSE 推送。
            </p>
            <!-- 走到这里必定带着令牌（决策 189）：没有令牌的码根本不会画出来 -->
            <div class="pair">
              <span class="pair-note">
                <!-- 隐喻首现翻译（决策 200，票 25）：本页第一次出现「值班长」，给一次平实说法。
                     行内全宽括号紧跟词后、与词同字号同色档（都在这一句里），页内不重复；
                     后面那处不再解释。 -->
                二维码已带上配对令牌：扫这一次，这台手机就能改任务、也能跟值班长（跟我对话的 AI）说话。
              </span>
              <button type="button" class="btn" disabled={reset} onclick={() => void doReset()}>
                {reset ? '正在重置…' : '重置配对'}
              </button>
            </div>
          </div>
        </div>

        {#if addresses.length > 1}
          <div class="alt">
            <div class="alt-head">其他网卡地址（首选不通时可换一个试试）</div>
            <ul class="alt-l">
              {#each addresses as a (a.url)}
                <li>
                  <button
                    type="button"
                    class="alt-item {a.url === selected ? 'on' : ''}"
                    onclick={() => (selected = a.url)}
                  >
                    <span class="iface">{a.interface}</span>
                    <span class="mono u">{a.url}</span>
                    {#if isPreferred(a)}<span class="tag">推荐</span>{/if}
                  </button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      {:else}
        <!-- 决策 189：有地址却没有令牌——不画那张扫了配不上的码，换成说清去哪台机器上打开。
             判定住在 `lib/sharePairing.ts::sharePanel`，它的模块注释写了这条为什么值得单独钉。 -->
        <section class="gate">
          {#if tokenState.kind === 'failed'}
            <div class="gate-head">读不到配对令牌</div>
            <p>
              这一页拿令牌时失败了：<span class="mono">{tokenState.message}</span
              >。没有令牌的二维码扫了也配不上，所以这里给的是原因而不是一张码。
            </p>
            <p class="note">先刷新这一页重试一次；还是失败的话，去跑服务的那台电脑上看日志。</p>
          {:else}
            <!-- 空态（票 13）：状态 → 下一步；「没有令牌的码扫了也配不上」必须说在明处。 -->
            <EmptyState
              state="二维码要在这台电脑本机上打开本页才拿得到"
              next="配对令牌只允许本机读取（这是它作为凭据的前提），而这一页现在是从 {origin} 打开的，读不到它。没有令牌的二维码扫了也配不上，所以这里不再画一张扫不出结果的码。"
            />
            <p class="note">
              在这台跑服务的电脑上打开
              <span class="mono">http://127.0.0.1:{info.port}/#/share</span>
              （桌面应用里就是顶栏的「手机访问」），那一页的二维码才带令牌，扫一次就配好。
            </p>
          {/if}
        </section>
      {/if}

      <p class="warn">
        ⚠ 同一网段的设备都能看本服务的只读页面（看板 / 会话 / 指标 / 本页）；
        改任务与跟值班长对话需要配对——二维码里那个令牌就是凭据。
        怀疑泄露时点「重置配对」，旧令牌立即失效，各设备重扫一次即可。
      </p>

      <!-- 决策 186：开了之后要能关回来，且说清这次绑定是谁定的 -->
      <div class="switch">
        <button type="button" class="btn" disabled={switching} onclick={() => void switchLan(false)}>
          {switching ? '正在改绑…' : '改回只绑本机（关掉手机访问）'}
        </button>
        <span class="switch-note">
          当前绑定 <span class="mono">{info.host}:{info.port}</span>，来自 <span class="mono"
            >{info.bind_source === 'startup'
              ? '启动参数'
              : info.bind_source === 'settings'
                ? '界面上的选择'
                : '配置文件'}</span
          >。
        </span>
      </div>
      {#if switchNote}<p class="note ok">{switchNote}</p>{/if}
      {#if switchError}<p class="note bad">{switchError}</p>{/if}
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
  /* 二维码牌：2px 描边盒 + 右侧地址栏（移动款纵向堆叠，见下方媒体查询） */
  .qrbox {
    display: flex;
    gap: 20px;
    align-items: flex-start;
    flex-wrap: wrap;
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 16px;
  }
  /* 二维码底色恒白：深色主题下反色二维码识别率差（扫描器依赖明暗对比）。
     这是 css-parity.test.ts 唯一 allowlist 的裸 #fff。 */
  .qr-qr {
    flex: none;
    width: 240px;
    height: 240px;
    background: #fff;
    border: 2px solid var(--pane);
    display: grid;
    place-items: center;
  }
  .qr-qr img {
    display: block;
  }
  .qr-side {
    flex: 1;
    min-width: 260px;
  }
  .chart-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 10px;
  }
  .port {
    color: var(--text-3);
    border: 2px solid var(--pane);
    padding: 0 5px;
  }
  .picked {
    background: var(--bg);
    border: 2px solid var(--pane);
    color: var(--text-hi);
    padding: 8px 10px;
    margin-bottom: 10px;
    word-break: break-all;
  }
  .qr-cap {
    margin-top: 12px;
    color: var(--text-3);
    line-height: 1.8;
  }
  /* 配对区（决策 182㉙）：与二维码同栏，读的次序是「先扫，扫完就配好了」 */
  .pair {
    margin-top: 12px;
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .pair .pair-note {
    margin-top: 0;
    flex: 1;
    min-width: 0;
    color: var(--text-3);
    line-height: 1.8;
    max-width: 62ch;
  }
  .alt {
    margin-top: 18px;
    border-top: 2px solid var(--wash);
    padding-top: 12px;
  }
  .alt-head {
    color: var(--text-3);
    letter-spacing: 0.08em;
    margin-bottom: 8px;
  }
  .alt-l {
    list-style: none;
  }
  .alt-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    border: 2px solid transparent;
    color: var(--text);
    padding: 3px 8px;
  }
  .alt-item:hover {
    background: var(--wash);
  }
  .alt-item.on {
    border-color: var(--go);
  }
  .iface {
    color: var(--text-3);
    min-width: 56px;
    flex: none;
  }
  .u {
    flex: 1;
    word-break: break-all;
  }
  .tag {
    flex: none;
    color: var(--go);
    border: 2px solid var(--go);
    padding: 0 5px;
  }
  /* 仅回环绑定 / 无地址：**中性档**的指引块（票 12：琥珀只出现在有东西要你处理的地方，
     入口闸的标题是「现在拿不到什么 + 下一步按哪颗钮」，回到亮档；无待办时不用告警色）。 */
  .gate {
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 16px;
    line-height: 1.85;
    color: var(--text-2);
  }
  .gate-head {
    color: var(--text-hi);
    font-size: 24px;
    margin-bottom: 10px;
  }
  .gate pre {
    background: var(--bg);
    border: 2px solid var(--pane);
    padding: 10px 12px;
    overflow-x: auto;
    margin: 8px 0 14px;
  }
  .gate code {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-hi);
  }
  .how {
    color: var(--text-3);
    margin: 12px 0 4px;
  }
  .note {
    margin-top: 14px;
    color: var(--text-3);
  }
  /* 改绑按钮行（决策 186）：一颗真的能按的钮 + 一句它意味着什么 */
  .switch {
    margin-top: 14px;
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .switch-note {
    color: var(--text-3);
    line-height: 1.8;
  }
  /* 改绑结果：成功与失败各自一档，不共用颜色（「响了」只在急停处，故这里用文字档） */
  .note.ok {
    color: var(--go);
  }
  .note.bad {
    color: var(--stop);
  }
  /* 启动期指定绑定的老办法：收进 details，不与那颗钮抢注意力 */
  .manual {
    margin-top: 14px;
    color: var(--text-3);
  }
  .manual summary {
    cursor: pointer;
    color: var(--text-2);
  }
  .hi {
    color: var(--text-hi);
  }
  /* 票 12 逐处判定：**保留琥珀**——开放局域网前要确认网段可信，这是要用户拍板的风险提示
     （不是「渲染出一个事实」），与入口闸标题那处不同。 */
  .warn {
    margin-top: 16px;
    border-top: 2px solid var(--wash);
    padding-top: 12px;
    color: var(--pending);
    line-height: 1.85;
  }

  /* 窄屏（<480px）：二维码与说明纵向堆叠；地址项改 44px 触控行（§5） */
  @media (max-width: 479px) {
    .page {
      padding: 16px 12px calc(48px + var(--safeb));
    }
    .qrbox {
      flex-direction: column;
      gap: 14px;
    }
    .qr-qr {
      align-self: center;
    }
    .qr-side {
      min-width: 0;
    }
    .alt-item {
      min-height: 44px;
      padding: 0 8px;
    }
  }
</style>
