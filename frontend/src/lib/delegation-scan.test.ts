import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

/**
 * @vitest-environment node
 *
 * 抽出去的判断**不能又被抄回来**（决策 251⑥）。
 *
 * 单测只能证明新 module 是对的，**证明不了组件还在用它**——组件一旦改回就地推导，
 * module 的测试照旧全绿，而重复回来了。本仓已经被这个形状咬过：`formatTokens` 至今在
 * `lib/pipeline.ts` 与 `lib/format.ts` 逐字重复两份，而 `lib/format.test.ts` 那趟
 * 「全站扫描重复的 locale 调用」没有覆盖到它。
 *
 * 扫描面是**源码文本**（纯静态，不跑 Svelte 编译器），属既有「静态扫描」家族：
 * `lib/talkLayout.test.ts` / `lib/copy-discipline.test.ts` / `theme/css-parity.test.ts`。
 *
 * **注释里的引用不参与断言**（下面的源码里就带着对 `lib/menuTrap` 的口头引用），
 * 故每条正向断言都锚在**真正的代码形状**上——函数定义、调用表达式、选择器字面量。
 */

const srcRoot = resolve(process.cwd(), 'src');

function read(rel: string): string {
  return readFileSync(resolve(srcRoot, rel), 'utf8');
}

/**
 * 把注释换成空格——**长度与行号不变**。本文件头那条「注释里的引用不参与断言」对正反
 * 两向都成立：口头引用既不该满足正向断言，也不该触发反向牙齿。
 */
const mask = (text: string, html: boolean): string => {
  const blank = (source: string, re: RegExp): string =>
    source.replace(re, (m) => m.replace(/[^\n]/g, ' '));
  let out = blank(text, /\/\*[\s\S]*?\*\//g);
  out = blank(out, /(?<!:)\/\/[^\n]*/g);
  if (html) out = blank(out, /<!--[\s\S]*?-->/g);
  return out;
};

describe('键盘陷阱只有一份（lib/menuTrap，决策 251⑤）', () => {
  const talk = read('routes/Talk.svelte');
  const topBar = read('components/layout/TopBar.svelte');

  it('两处弹层都从 lib/menuTrap 取判据', () => {
    for (const [name, src] of [
      ['routes/Talk.svelte', talk],
      ['components/layout/TopBar.svelte', topBar],
    ] as const) {
      expect(src, `${name} 没有引入 createMenuTrap`).toContain('createMenuTrap(');
      expect(
        src.match(/import \{[^}]*createMenuTrap[^}]*\} from '\.\..*\/lib\/menuTrap'/)?.[0],
        `${name} 的 createMenuTrap 不是从 lib/menuTrap 来的`,
      ).toBeTruthy();
    }
  });

  it('两处都不再自己定义那两个 window 监听（搬进 helper 后它们已无处可定义）', () => {
    for (const [name, src] of [
      ['routes/Talk.svelte', talk],
      ['components/layout/TopBar.svelte', topBar],
    ] as const) {
      for (const fn of ['onWindowKey', 'onWindowClick']) {
        expect(src.includes(`function ${fn}`), `${name} 又定义了 ${fn}——重复回来了`).toBe(false);
      }
    }
  });

  it('两处都不再自己实现绕回公式（它归 wrapIndex）', () => {
    const wrapFormula = /\(\([a-z]+ % [a-z]+\) \+ [a-z]+\) % [a-z]+/;
    for (const [name, src] of [
      ['routes/Talk.svelte', talk],
      ['components/layout/TopBar.svelte', topBar],
    ] as const) {
      expect(wrapFormula.test(src), `${name} 里又出现了绕回公式`).toBe(false);
    }
    // 反向：那一份在 module 里，且 module 有单测钉着
    expect(wrapFormula.test(read('lib/menuTrap.ts')), 'wrapIndex 里该有那条公式').toBe(true);
  });

  it('两处都不再手搓「可聚焦项」查询（选择器作为 opts 传进 helper）', () => {
    expect(talk.includes("querySelectorAll<HTMLElement>('button[data-menu-item]")).toBe(false);
    expect(topBar.includes("querySelectorAll<HTMLAnchorElement>('a.dd-item')")).toBe(false);
    // 选择器本身仍在两处各自的 opts 里——**这是刻意的**（Talk 有禁用项要跳过，顶栏全是链接）
    expect(talk).toContain("itemSelector: 'button[data-menu-item]:not([disabled])'");
    expect(topBar).toContain("itemSelector: 'a.dd-item'");
  });
});

describe('工位灯聚合只有一处判据（lib/pipeline，决策 251②）', () => {
  const talk = read('routes/Talk.svelte');
  const boardColumn = read('components/board/BoardColumn.svelte');
  const pipeline = read('lib/pipeline.ts');

  it('看板列头与对讲台值班板都调 aggregateStationState', () => {
    for (const [name, src] of [
      ['routes/Talk.svelte', talk],
      ['components/board/BoardColumn.svelte', boardColumn],
    ] as const) {
      expect(src, `${name} 没有调 aggregateStationState`).toContain('aggregateStationState(');
    }
    expect(pipeline).toContain('export function aggregateStationState');
  });

  it('两处都不再就地推那一串优先序（搬进 helper 后它们已无处可推）', () => {
    // 老形状：Talk 的 `state: pen ? 'warn' : live ? 'run' ...`、列头的 `hasPending ? 'warn' ...`
    expect(talk).not.toContain("state: pen ? 'warn'");
    expect(talk).not.toContain("'warn' : live ? 'run'");
    expect(boardColumn).not.toContain("hasPending ? 'warn'");
    expect(boardColumn).not.toContain("hasRunning ? 'go' : hasFailed");
  });

  it('失败灯那一档真的接了（`.blamp.x` 是值班板的失败变体）', () => {
    expect(talk, 'Talk 的样式里该有 .blamp.x').toContain('.blamp.x');
    expect(talk, 'Talk 该把 stop 映到 x').toContain("case 'stop':\n        return 'x';");
  });

  it('`.blamp.x` 的取色走 --stop（与看板失败列同一个 token，不另造色）', () => {
    const block = /\.blamp\.x \{[^}]*\}/.exec(talk)?.[0] ?? '';
    expect(block, '没找到 .blamp.x 的规则块').toBeTruthy();
    expect(block, '.blamp.x 该取 --stop').toContain('var(--stop)');
    // 不许在这里塞字面量色值——决策 169 的 token 纪律（theme/css-parity.test.ts 同款）
    expect(/#[0-9a-fA-F]{3,8}/.test(block), '.blamp.x 里出现了裸十六进制色值').toBe(false);
  });

  it('在跑那一档的行提亮没丢（`.brow.hot` 必须有生产者，否则它成死 CSS）', () => {
    // 词表从自造的 `'run'` 换回契约的 `'go'` 时差点把这一档整个漏掉：行 class 不再返回
    // `hot`，而 `.brow.hot .bnm { color: var(--text-hi) }` 还留在样式里——**看板与值班板
    // 就此对「在跑」有两种画法**。决策 251 说「唯一的行为变化是失败灯」，这一档不许动。
    expect(talk, 'crewRowClass 漏了 go → hot').toContain("if (state === 'go') return 'hot';");
    expect(talk, '.brow.hot 的规则还在吗（不在的话上一条就悬空了）').toContain('.brow.hot .bnm');
  });
});

describe('回合构造只有一份（lib/talkTurns，票 02 / 决策 251⑥）', () => {
  const talk = read('routes/Talk.svelte');
  const talkTurns = read('lib/talkTurns.ts');

  it('Talk 从 lib/talkTurns 取构造（import 是真的，不是口头引用）', () => {
    expect(talk, 'Talk 没有调 buildTurns').toContain('buildTurns(');
    expect(
      talk.match(/import \{[^}]*buildTurns[^}]*\} from '\.\..*\/lib\/talkTurns'/)?.[0],
      'Talk 的 buildTurns 不是从 lib/talkTurns 来的',
    ).toBeTruthy();
    expect(talkTurns, 'module 里该有导出的 buildTurns').toContain('export function buildTurns(');
  });

  it('Talk 不再内联正文哨兵（`startsWith` 认轮次是决策 252 判过的事，前端不许解析正文）', () => {
    // 票 02 明确的两条：失败轮与值守播报的哨兵判定归后端字段（kind / proactive），
    // 界面这半在 252 之后本就不该再有；此处钉的是「抽走之后别又抄回来」。
    expect(talk).not.toContain('startsWith(FOREMAN_FAILED_TURN_MARK');
    expect(talk).not.toContain('startsWith(FOREMAN_WATCH_MARK');
    // 反向：module 在场，且它读的是字段（决策 252），不是哨兵
    expect(talkTurns).toContain('m.kind');
    expect(talkTurns).toContain('m.proactive');
    // 调用形状才算数：docblock 里口头引用 `startsWith` 是叙事（「此前两个判定点迟早不一致」）
    expect(talkTurns).not.toContain('startsWith(');
  });

  it('Talk 不再自己排那条 stamped.sort（排序与合并在行的关系上，归 module）', () => {
    expect(talk, 'Talk 里又出现了 stamped——合并逻辑回来了').not.toContain('stamped');
    expect(talkTurns, 'module 里该有那条 sort').toContain('stamped.sort(');
    // rank 三元式（同刻兜底次序）也只在 module 里
    expect(talk, 'Talk 里又出现了 rank 三元式').not.toContain("m.kind === 'mine' ? 0");
    expect(talkTurns).toContain("m.kind === 'mine' ? 0");
  });

  it('配对判据按后端 kind、不按报文字样（票 04 / 决策 259）', () => {
    // 旧形状两件都不许回来：按正文判的谓词、以及把它当回调传的调用点
    expect(talk, 'Talk 又开始按报文字样判配对了').not.toContain(".includes('还没配对')");
    expect(talk, 'needsPairing 谓词回来了').not.toContain('function needsPairing');
    // 判据收在 lib、吃后端给的 kind；Talk 的两处 catch 调它（判在 ApiError 还在手的那层）
    expect(talk, 'Talk 该调 isPairingRequired').toContain('isPairingRequired(err)');
    expect(
      talk.match(/import \{[^}]*isPairingRequired[^}]*\} from '\.\..*\/lib\/sharePairing'/)?.[0],
      'isPairingRequired 不是从 lib/sharePairing 来的',
    ).toBeTruthy();
    const share = read('lib/sharePairing.ts');
    expect(share, '判据该钉 kind').toContain("err.kind === 'pairing_required'");
    // 注释里的旧形状引用不参与断言（本文件开头的约定），剥掉块注释再扫 includes
    const shareCode = share.replace(/\/\*[\s\S]*?\*\//g, '');
    expect(shareCode, 'lib 里又出现了按字样判').not.toContain("includes('还没配对')");
    // buildTurns 只收上游判好的布尔，不再收回调
    expect(talk).toContain('pairingNeeded: sendPairingNeeded');
    expect(talkTurns, 'module 该收布尔').toContain('pairingNeeded: boolean');
    expect(talkTurns, 'module 不许再调报文谓词').not.toContain('needsPairing(');
  });
});

describe('动作身份只有一把尺子（lib/actions actionKey，票 05）', () => {
  const boardStore = read('stores/board.svelte.ts');
  const taskCard = read('components/board/TaskCard.svelte');
  const talk = read('routes/Talk.svelte');

  it('生产者与三个消费者都用 actionKey，且 import 都指向 lib/actions', () => {
    expect(boardStore, 'board store 该用 actionKey 产忙键').toContain(
      'actionKey(action, opts.cursorId)',
    );
    expect(taskCard, 'TaskCard 该按 actionKey 判忙').toContain('actionBusy === actionKey(action, cursorId)');
    expect(talk, 'Talk 该按 actionKey 判忙').toContain('actionKey(a, cursorId)');
    for (const [name, src] of [
      ['stores/board.svelte.ts', boardStore],
      ['components/board/TaskCard.svelte', taskCard],
      ['routes/Talk.svelte', talk],
    ] as const) {
      expect(
        src.match(/import \{[^}]*actionKey[^}]*\} from '[^']*lib\/actions'/)?.[0],
        `${name} 的 actionKey 不是从 lib/actions 来的`,
      ).toBeTruthy();
    }
  });

  it('两段式与手工桥接不再出现（旧拼法三处都清零）', () => {
    expect(boardStore, 'store 又写回两段式了').not.toContain('${taskId}:${action.action}');
    expect(taskCard, 'TaskCard 的两段式桥接回来了').not.toContain('${task.id}:${action.action}');
    expect(taskCard, 'TaskCard 的截断版桥接回来了').not.toContain('${action.action}:${cursorId ??');
    expect(talk, 'Talk 又按 taskId 前缀判忙了').not.toContain('${task.id}:${a.action}');
  });
});

describe('超时判据按 kind、不摸正文（票 06，决策 259 的延伸）', () => {
  const foreman = read('realtime/foreman.ts');
  // 发送编排自决策 354② 起住在 store：超时判据的接线点（趁 `ApiError` 还在手判好）随发送
  // 搬走——扫描面跟着判据走（**换文件不等于放松**，与决策 275 / 354① 同一手法），
  // 并反过来钉住页面里不再有第二处。
  const store = read('stores/talk.svelte.ts');
  const talk = mask(read('routes/Talk.svelte'), true);
  const client = read('api/client.ts');

  it('realtime/foreman.ts 不再有 startsWith(\'请求超时\')，判据钉在 kind 上', () => {
    // 注释里的旧形状引用不参与断言（本文件开头的约定），剥掉块注释再扫
    const foremanCode = foreman.replace(/\/\*[\s\S]*?\*\//g, '');
    expect(foremanCode, '按正文判超时的旧谓词回来了').not.toContain("startsWith('请求超时')");
    expect(foremanCode, 'isTimeoutMessage 回来了').not.toContain('isTimeoutMessage');
    expect(foreman, '判据该钉 kind').toContain('err.kind === KIND_REQUEST_TIMEOUT');
    // kind 常量来自 mapRequestError 那一侧（生产者与消费者共用一枚，不许这里再抄字面量）
    expect(client).toContain('export const KIND_REQUEST_TIMEOUT');
    expect(client, '超时那条构造该把 kind 作为第三参传进去').toContain(', KIND_REQUEST_TIMEOUT)');
  });

  it('发送编排趁 ApiError 还在手把判好的布尔交给 failureNotice（判上移、拼接留下游）', () => {
    expect(store, '该调 isRequestTimeout').toContain('isRequestTimeout(err)');
    // 判好的布尔**存进一个具名变量**再交给下面的分支（决策 260）：`finally` 里还要
    // 用它决定接不接那一轮，故不能在实参位置上判一次完事。
    expect(store, '该把判好的布尔存下来').toContain('timedOut = isRequestTimeout(err)');
    expect(
      store.match(/import \{[^}]*isRequestTimeout[^}]*\} from '\.\..*\/realtime\/foreman'/)?.[0],
      'isRequestTimeout 不是从 realtime/foreman 来的',
    ).toBeTruthy();
    expect(talk, '页面里不该再自己判一次超时（判据只有一处）').not.toContain('isRequestTimeout');
  });

  it('本地放弃走安静态：超时不落失败轮、接手无条件（决策 288 / 票 05）', () => {
    expect(store, '超时该走安静态（不落失败轮）').toContain(
      'this.stream = quietAfterLocalGiveUp(this.stream)',
    );
    expect(store, '接手该走 followAfterGiveUp（无条件，不等 stale 读数）').toContain(
      'this.followAfterGiveUp(maxLedgerId(session.messages ?? []))',
    );
    expect(
      store.match(
        /import \{[^}]*quietAfterLocalGiveUp[^}]*\} from '\.\..*\/realtime\/foreman'/,
      )?.[0],
      'quietAfterLocalGiveUp 不是从 realtime/foreman 来的',
    ).toBeTruthy();
    // 非超时的失败仍走失败轮（网络不通 / 配对 403 是真失败，一个字都不动）。
    expect(store, '非超时失败照旧落失败轮').toContain('failForemanStream(');
    expect(talk, '页面里不该再有那两样（接线跟着发送走了）').not.toContain(
      'quietAfterLocalGiveUp',
    );
  });
});

describe('跟的那一轮的收场分三支（决策 260 裁决③，票 in-flight-turn 01；收口自决策 275 起在 store）', () => {
  const talk = read('routes/Talk.svelte');
  const talkCode = mask(talk, true);
  const foreman = read('realtime/foreman.ts');
  // 三支的收口自决策 275 起住在 store（在飞现场随页面来去，收口自然也跟着它走）；
  // 决策 354① 之后 reload 与 syncFollowing 同住 store，决策 354② 之后**发送的收尾**
  // 也搬了进来——页面这一侧一处都不剩。扫描面因此整段跟着判据走——**换文件不等于放松**：
  // 下面每条断言一字不改地要求同一个形状，并反过来钉住页面里没有第二处。
  const store = read('stores/talk.svelte.ts');

  it('落地哨把收场交给纯函数，不再就地「一把梭清字」', () => {
    // 曾经的形状：`const landed = turnLanded(...) || !turn_in_flight; if (!landed) return;`
    // 然后无条件 `emptyForemanStream()`——死轮（没换行、服务端也不在跑了）恰好也走那一支，
    // 已经收到的半截字随之被清掉、那一轮从时间线上整段消失。判据收进 `resolveFollowOutcome`
    // 之后，三支各自决定「清 / 留 + 以何姿态留」。
    expect(store, '落地哨该按纯函数的三支分支').toContain('resolveFollowOutcome(');
    expect(store, '死轮那一支要留着半截字').toContain('FOREMAN_LOST_TURN_SUFFIX');
    // 两支要显式分出来（`settled` 是兜底那一支，故它没有字面量）：
    // keep = 还在跑、什么都不动；lost = 死轮、留字 + 一句说明
    expect(store, '少了「继续跟」那一支').toContain("outcome.kind === 'keep'");
    expect(store, '少了「死轮」那一支').toContain("outcome.kind === 'lost'");
    // 死轮那一支必须留字（走 failForemanStream）、且不许顺手清掉（emptyForemanStream）
    const lostBranch = store.slice(store.indexOf("outcome.kind === 'lost'"));
    const lostBody = lostBranch.slice(0, lostBranch.indexOf('return;'));
    expect(lostBody, '死轮该按「保留半截字」的姿态收').toContain('failForemanStream');
    expect(lostBody, '死轮不许清字').not.toContain('emptyForemanStream');
    // 判据不许再就地写回（那两个读数各自答一半，合起来才是三支）
    expect(store, '落地判据不该就地进行').not.toContain(
      'turnLanded(payload.messages ?? [], anchor) || !payload.turn_in_flight',
    );
    // 页面只**接线**：收口这件事走 store 的同一个入口（页面里不许再自己判一遍三支）。
    // 决策 354① 之后 reload 与 syncFollowing 同住 store（落地后的重读不再经过页面）；
    // 决策 354② 之后发送的收尾也搬了进来——原先留在页面上的那一处 `talk.syncFollowing(`
    // 随之退场，故两条正向断言一并锚在 store。
    expect(store, '发送收尾该把收口交给同一入口').toContain('this.syncFollowing(session)');
    expect(store, 'reload 之后的收口也走同一入口').toContain('this.syncFollowing(payload)');
    expect(talkCode, '页面里不该再自己算三支').not.toContain("outcome.kind === 'lost'");
    expect(talkCode, '页面里不该再接那一轮（收口只有 store 一处）').not.toContain('syncFollowing');
  });

  it('三支的判据与文案住在纯函数模块里（可单测、不触 DOM）', () => {
    expect(foreman, '判据该在模块里').toContain('export function resolveFollowOutcome');
    expect(foreman, '死轮的说明该在模块里').toContain('export const FOREMAN_LOST_TURN_SUFFIX');
    // 「任何一支都不许清掉已出现的文字」这条纪律的落点：死轮走 failForemanStream（留字），
    // 不是 emptyForemanStream（清字）
    expect(foreman, '死轮该按「保留」的姿态收').toContain('failForemanStream');
  });
});

describe('发送编排只有一处（决策 354②：串台守卫收成 claim，排水环住 store）', () => {
  const talk = mask(read('routes/Talk.svelte'), true);
  const store = read('stores/talk.svelte.ts');

  it('串台守卫是一条判据、一个出口：`claim()` 的闭包；页面里不许再有手写比对', () => {
    // 收口之前是五种手写、三种锚点拼法（wanted / originSid / gen）——漏一处就是
    // 「对讲台出现非本次会话的内容」那个历史根因。判据现在只有这一处。
    expect(store, '守卫该有唯一定义').toContain('claim(): () => boolean');
    expect(store, '发送那条路该认下守卫').toContain('let mine = this.claim();');
    expect(store, '两处取数各认一次（reload / loadEarlier）').toContain('const mine = this.claim();');
    expect(talk, '页面里不该再有手写的在途比对').not.toContain('talk.sessionId !==');
    // 旧拼法一并退场（换文件不等于放松，反向也要钉）：
    expect(store, 'reload 里那两处旧的 `wanted` 比对该没了').not.toContain('sessionId !== wanted');
    expect(store, 'loadEarlier 里那个 `gen` 记号该没了').not.toContain('gen !== this.sessionId');
  });

  it('「在飞」的判据也只有一处（`inFlight`）：入队与出队问的是同一个问题', () => {
    // 页面原先自己判一遍（send 里那段三元）、那条出队 effect 再判一遍——两处迟早分叉。
    expect(store, '该有唯一定义').toContain('private get inFlight(): boolean');
    expect(store, '入队读它').toContain('if (this.inFlight && sid) {');
    expect(store, '出队读它').toContain('if (this.inFlight) return;');
  });

  it('排水环住 store：由 App 起收，判据读 store 自己的读数；页面里没有出队这一回事', () => {
    expect(store, '排水环该有起收两端').toContain('startQueueDrain()');
    expect(store, '随 App 起').toContain('this.startQueueDrain();');
    expect(store, '随 App 收').toContain('this.stopQueueDrain();');
    // 页面那个 effect 的两个额外读数在 store 里的等价物：watchMode / archivedOpen
    expect(store, '值守账不排水').toContain("this.kind === 'watch'");
    expect(store, '归档班次不排水').toContain('archived_at != null');
    expect(talk, '页面里不该再自己出队').not.toContain('takeQueued(');
    expect(talk, '页面里不该再有那条出队 effect 的判据').not.toContain('queueHeld[sid]');
  });
});

/**
 * 在飞轮的渲染键（决策 354③）：首条带 `ledger_id` 的事件到达即改用 `m<ledger_id>`，
 * 于是「在飞」与「落地」两态**共用同一个键**——折叠态搬运机（`settlingTurn` +
 * `carryLiveStepOpen` + `carryLiveTurnOpen` + 组件 effect）整段退场。
 *
 * 反面牙齿（换文件不等于放松）：这四件在页面、store、模块三处都不许再出现；页面里
 * 连更宽的 `sessionId !==` 拼法也不许有——票 02 那条已知边界（`liveSid` 会误伤更宽的
 * 反向扫描）随本票一起消失，故这里把扫描面放宽到它能放宽的全部。
 */
describe('在飞轮键只有一处（决策 354③：搬运机整段退场）', () => {
  const talk = mask(read('routes/Talk.svelte'), true);
  const store = mask(read('stores/talk.svelte.ts'), false);
  const turns = mask(read('lib/talkTurns.ts'), false);

  it('键的判据只有一处：回合构造里那一个派生（`liveTurnKey`）', () => {
    expect(turns, '该有唯一定义').toContain('function liveTurnKey(');
    expect(turns, '在飞轮那一支读它').toContain('const key = liveTurnKey(base, stream.events);');
    expect(turns, '`ledger_id` 是那条判据的输入').toContain('if (ev.ledger_id != null) return');
  });

  it('同一轮只摆一遍：落地行在场时在飞轮整条退场（同键不许出现两次）', () => {
    // 两态同键带来的一条新守卫（票 03 注记 ⑤，e2e 闸门逼出来的）：收口那一拍
    // （`reload` 已写落地行、`settleTurn` 还没倒空现场）两条本会同时出现，
    // 而 `Talk.svelte` 的 keyed each 对同键抛错（`svelte.dev/e/each_key_duplicate`）。
    expect(turns, '在飞轮入栈前要查同键那一行在不在').toContain(
      'if (!out.some((t) => t.key === key)) {',
    );
  });

  it('搬运机三处都不在了（页面 / store / 模块）', () => {
    for (const [name, src] of [
      ['routes/Talk.svelte', talk],
      ['stores/talk.svelte.ts', store],
      ['lib/talkTurns.ts', turns],
    ] as const) {
      for (const gone of ['carryLiveStepOpen', 'carryLiveTurnOpen', 'settlingTurn']) {
        expect(src.includes(gone), `${name} 里又出现了 ${gone}——搬运机回来了`).toBe(false);
      }
    }
    // 页面那一对暂存（`liveSeen` / `liveSid`）与它那段 effect 一并退场
    expect(talk, '页面里不该再有收口搬运的暂存').not.toContain('liveSeen');
    expect(talk, '页面里不该再有那一班记号').not.toContain('liveSid');
  });

  it('页面里连更宽的 `sessionId !==` 拼法也没有了（票 02 的已知边界随本票消失）', () => {
    expect(talk, '页面里不该再有手写的在途比对').not.toMatch(/sessionId\s*!==/);
  });
});

describe('中断标记只有一个来源（票 03：台账字段 → 回合构造 → 界面）', () => {
  const talk = mask(read('routes/Talk.svelte'), true);
  const turns = mask(read('lib/talkTurns.ts'), false);
  const store = mask(read('stores/talk.svelte.ts'), false);
  const foreman = mask(read('realtime/foreman.ts'), false);

  it('标记按字段渲染：回合构造只搬 `interrupted_at`，页面只看 `turn.interruptedAt`', () => {
    // 判据在后端给的字段上（决策 252 同一条边界）：页面自己从 status / 正文推「已中断」
    // 就是第二份判定点——两份迟早不一致，而这条标记说的正是「库里那条行是什么状态」。
    expect(turns, '回合构造该把字段原样搬过去').toContain(
      'interruptedAt: m.interrupted_at ?? null',
    );
    expect(talk, '页面该按字段渲染').toContain('{#if turn.interruptedAt}');
    // 标记与时刻一起摆出来（票 03 的 checklist：带标记 + 中断时刻）——时刻走
    // `formatDateTime`（时间格式唯一出处，format.test.ts 那条静态扫描钉着），不裸 toLocaleString
    expect(talk, '标记要带上时刻').toContain('已中断 · {formatDateTime(turn.interruptedAt)}');
    // 反向牙齿：页面里不许再长出第二份推导
    expect(talk, '页面不该自己判行状态').not.toContain("status === 'interrupted'");
    expect(talk, '页面不该从正文里抠标记').not.toContain('interrupted_at');
  });

  it('store 与判据模块都不合成「已中断」——标记的来路只有台账字段那一条', () => {
    // 改回就地合成（在 store / foreman 里自己拼一条中断轮或标记）即红。
    //
    // **读纯函数判好的布尔不算合成**（2026-09-29，票 04 中断扣队列）：`outcome.interrupted`
    // 在 `resolveFollowOutcome` 里从 `anchored.status`（台账字段）算出，判定点仍只有那一处；
    // store 这一侧的红线是**再算一遍**（摸 raw status）与**带界面文案**——原断言
    // `not.toContain('interrupted')` 连字段名一起禁，把「接线读判据」误伤成「就地合成」。
    expect(store, 'store 不该自己重判行状态').not.toContain("status === 'interrupted'");
    expect(store, 'store 不该摸原始字段').not.toContain('interrupted_at');
    expect(store, 'store 不该出现界面文案').not.toContain('已中断');
    expect(store, '中断扣队列该读纯函数判好的布尔').toContain('outcome.interrupted');
    expect(foreman, '三支判据只认 status 字段，不带界面文案').not.toContain('已中断');
    // 中断行落进哪一支由纯函数判（就地收口那条：status 离开 in_flight 就是落地），
    // 单测 foreman.test.ts 钉着 interrupted → settled 的实际取值。
    expect(foreman, '中断行该在纯函数里被判成收口').toContain("anchored.status !== 'in_flight'");
  });
});

describe('值守台账与对讲台同源受益（票 07：同组件同 store，不写分支逻辑）', () => {
  const app = mask(read('App.svelte'), true);
  const talk = mask(read('routes/Talk.svelte'), true);

  /**
   * 取两个锚点之间的源码。锚点缺席时返回 `null`，随后的断言当场变红——
   * 静态扫描最怕的不是误报，是**锚点悄悄挪走后整段空转地绿着**。
   */
  const region = (src: string, from: string, to: string): string | null => {
    const a = src.indexOf(from);
    if (a < 0) return null;
    const b = src.indexOf(to, a + from.length);
    return b < 0 ? null : src.slice(a, b);
  };

  it('两条路由渲染同一个 Talk 组件——watch 只是一个传参，不是第二份实现', () => {
    expect(app, '对讲台该渲染 Talk').toContain('<Talk />');
    expect(app, '值守台账该渲染同一个 Talk').toContain('<Talk watch />');
    expect(app, 'Talk 只从 routes/Talk.svelte 导入一次').toContain(
      "import Talk from './routes/Talk.svelte'",
    );
    expect(app, '不该存在第二份值守实现').not.toContain('TalkWatch');
  });

  it('回看的三处都不按账本分叉（loadEarlier / 中断标记 / 归档开关——分支是分叉的起点）', () => {
    // 向上加载：两本账走同一条路径，唯一差异是 `kind`（`?kind=`，同源判据里明写的那
    // 「一个参数」）。决策 354① 之后取数在 store（两本账同一份实现，比「同一组件里
    // 不分支」更强）；页面只剩滚动几何的包装——所以正向认 store 里的 kind、反向拒
    // 页面里的 watchMode。
    const store = read('stores/talk.svelte.ts');
    const earlier = region(talk, 'async function loadEarlier', 'function onTimelineScroll');
    expect(earlier, 'loadEarlier 包装锚点该在 Talk.svelte 里').not.toBeNull();
    const storeEarlier = region(store, 'async loadEarlier', 'async switchTo');
    expect(storeEarlier, 'loadEarlier 取数锚点该在 store 里').not.toBeNull();
    expect(storeEarlier, '取数差异只许走 kind（?kind=）').toContain('this.kind');
    expect(storeEarlier, '向上加载不许按账本分叉').not.toContain('watchMode');
    expect(earlier, '向上加载不许按账本分叉').not.toContain('watchMode');
    // 「能不能翻」与「滚动几何」两半各在哪（决策 354①）：一半落进 store 就是把手伸进
    // DOM（它够不着），另一半落进页面就是第二个判据点——store 那两道早退已经在判了。
    expect(earlier, '滚动几何该留在页面（store 够不着 DOM）').toContain('scrollTop');
    expect(storeEarlier, '向上加载不摸滚动——那一格是页面的').not.toContain('scrollTop');

    // 中断标记：同一行渲染、同一句文案，值守账里长得一模一样。
    const cut = region(talk, '{#if turn.interruptedAt}', '{/if}');
    expect(cut, '中断标记块该在 Talk.svelte 里').not.toBeNull();
    expect(cut, '中断标记不许按账本分叉').not.toContain('watchMode');

    // 归档开关住在 `{#if !watchMode}` 动作门**外**（新班次 / 改名 / 归档才是门内的）：
    // 挪进那道门，值守账当场失去翻归档的口子——而 e2e 只在桌面视口断言它在场。
    const toggleAt = talk.indexOf('class="runchip arch-toggle"');
    const gateAt = talk.indexOf('{#if !watchMode}');
    expect(toggleAt, 'arch-toggle 锚点该在 Talk.svelte 里').toBeGreaterThan(-1);
    expect(gateAt, '{#if !watchMode} 动作门该在 Talk.svelte 里').toBeGreaterThan(-1);
    expect(toggleAt, '归档开关在 watch 门之外——值守账同样开得开').toBeLessThan(gateAt);
  });
});
