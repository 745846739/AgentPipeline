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
  const talk = read('routes/Talk.svelte');
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

  it('Talk 趁 ApiError 还在手把判好的布尔交给 failureNotice（判上移、拼接留下游）', () => {
    expect(talk, 'Talk 该调 isRequestTimeout').toContain('isRequestTimeout(err)');
    // 判好的布尔**存进一个具名变量**再交给 `failureNotice`（决策 260）：`finally` 里还要
    // 用它决定那条本地失败轮退不退场（超时那一类交棒给「跟」），故不能在实参位置上判一次完事。
    expect(talk, '该把判好的布尔存下来').toContain('timedOut = isRequestTimeout(err)');
    expect(talk, '该把那个布尔传给 failureNotice').toContain(
      'failureNotice((err as Error).message, timedOut)',
    );
    expect(
      talk.match(/import \{[^}]*isRequestTimeout[^}]*\} from '\.\..*\/realtime\/foreman'/)?.[0],
      'isRequestTimeout 不是从 realtime/foreman 来的',
    ).toBeTruthy();
  });
});
