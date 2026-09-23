/**
 * @vitest-environment node
 *
 * 跨语言共享表的 **Node 侧那一半**（票 mirror-contract/03，决策 253 ②）。
 *
 * 问题：前端手抄了四份**规格**（不是成员集），四份都没有测试读 Rust——
 * `lib/stageConfigs.ts` 的格子顺序（14 个配置键）、哪些键不是真实阶段、
 * `lib/stewardship.ts` 的终态集、`lib/pipeline.ts::pendingLabel` 的展示词表。
 * 两处已经登记为险的注释各自承诺「与后端同步」，而**两侧都只有注释，没有机器检查**：
 * 不同步的表现是「界面上填好、保存被 400 拒掉」，而 400 的理由写着「未知阶段」——
 * 看的人只会当成界面 bug 去查前端。
 *
 * 修法：`tests/fixtures/frontend_spec_tables.json` 把两侧钉在一起。本文件断言**前端那一半**，
 * Rust 侧表测试断言**后端那一半**（`types.rs::shared_spec_tables_match_the_backend_spec`），
 * 两侧读同一份、同一断言方向。
 *
 * 与 `enumMembersFixture.test.ts` 的分工：那份管「有哪些值」（成员集，Rust 导出），
 * 这份管「按什么次序 / 归哪一组 / 哪几个是终态」（规格，手写）。红的症状不同，故分开。
 *
 * **判据走公开接口，不为测试导出内部常量**：`PSEUDO_KEYS` 与 `TERMINAL_STATUSES` 都是
 * 模块私有的，这里分别经 `isPseudoStage` 与 `stewardshipFace` 断言——它们正是那两份私有表
 * 的**唯一消费者**，故经它们断言的强度与直接比表相同，且不为了让测试够得着而放宽接口。
 *
 * **跑在 node 环境**（照 `hostPolicyFixture.test.ts` 先例）：要按 `import.meta.url` 定位仓库根
 * 的 fixture，且不碰 DOM。
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

import { PENDING_KIND_MEMBERS } from '../api/types';
import type { TaskStatus } from '../api/types';
import { readFixture } from './fixtures';
import { pendingLabel } from './pipeline';
import { isPseudoStage, STAGE_KEYS } from './stageConfigs';
import { stewardshipFace } from './stewardship';

/**
 * 从 `stageConfigs.ts` 的**源文本**读出私有的 `PSEUDO_KEYS` 成员集。
 *
 * 为什么读源文本而不是把那个 Set 导出：它是模块内部实现，为了测试把它变成公开 API 是为测试
 * 放宽接口（本仓不收这一口）。而「集合相等」这条断言需要一个能枚举的清单——`isPseudoStage`
 * 只答「这一个是不是」，枚举不出来，故单靠它挡不住「集合里多出一个前端不展示的键」。
 * 读源文本是本仓已有的做法（照 `lib/talkLayout.test.ts` / `behavior-map.test.ts` 的静态扫描）。
 */
function pseudoKeysFromSource(): Set<string> {
  const source = readFileSync(
    new URL('./stageConfigs.ts', import.meta.url),
    'utf8',
  );
  const block = source.match(/const PSEUDO_KEYS = new Set\(\[([\s\S]*?)\]\);/);
  if (!block) {
    throw new Error('stageConfigs.ts 里找不到 `const PSEUDO_KEYS = new Set([...])`——扫描式过期了');
  }
  const keys = new Set<string>();
  for (const m of block[1].matchAll(/'([^']+)'/g)) keys.add(m[1]);
  if (keys.size === 0) throw new Error('解析 PSEUDO_KEYS 得到空集——扫描式过期了');
  return keys;
}

const specFixture = readFixture<{
  stage_keys: string[];
  pseudo_keys: string[];
  terminal_statuses: string[];
  user_decision_context_kinds: string[];
}>('frontend_spec_tables.json');

/**
 * `pendingLabel` 覆盖的 pending 种类取自**成员表**（`enum_members.json`），不取自本规格表。
 *
 * 规格表里再抄一份 `pending_kinds` 就是**第三份**副本（枚举一份、前端联合一份、规格表一份）
 * ——而决策 253② 的判据正是「枚举推得出来的东西不手抄」。这里缺的是「顺序」，而顺序对
 * `pendingLabel` 无意义（它是个 switch），故直接读成员表。
 */
const membersFixture = readFixture<{ pending_kind: string[] }>('enum_members.json');

/** `pendingLabel` 认不出一个种类时返回它自己（`default: return reason.type`）。 */
const UNKNOWN_KIND = '__不存在的种类__';

describe('规格表（票 mirror-contract/03，与 Rust 规格同源）', () => {
  it('fixture 形状完好（两侧的断言对象还在）', () => {
    expect(specFixture.stage_keys.length).toBe(14);
    expect(specFixture.pseudo_keys.length).toBe(4);
    expect(specFixture.terminal_statuses.length).toBe(3);
    // 必需行按名钉住（与 Rust 表测试同一把尺）：只数行数时随便塞行多余值也能过。
    for (const required of ['done', 'merge', 'foreman', 'project_analysis']) {
      expect(specFixture.stage_keys, `缺必需行 ${required}`).toContain(required);
    }
    // `foreman` **不是**真实阶段，但它**在**配置键里（决策 182①）——两处必须同时成立。
    expect(specFixture.stage_keys).toContain('foreman');
    expect(specFixture.pseudo_keys).toContain('foreman');
    // 丢掉一行的两个方向都要在这里现形（否则下面那条遍历会**空转变绿**）：
    // 少一个伪键 → Rust 侧的表测试与 app 的 `PSEUDO_STAGE_KEYS` 断言变红；
    // 少一个 user_decision 子类 → Rust 侧的 `user_decision_context_for` 反向断言变红。
    // 这里再按名钉几条，让**前端这一侧**也拦得住截断。
    expect(specFixture.pseudo_keys).toContain('conflict_check');
    expect(specFixture.pseudo_keys).toContain('project_analysis');
    for (const required of [
      'duplicate_risk',
      'dirty_worktree',
      'gate_recheck',
      'test_design_input_insufficient',
    ]) {
      expect(specFixture.user_decision_context_kinds, `缺必需行 ${required}`).toContain(required);
    }
  });

  it('STAGE_KEYS 逐项等于表里的 stage_keys（顺序也 pin）', () => {
    // 顺序是规格：它决定设置页的格子次序，而后端推不出来（决策 253②）。
    expect([...STAGE_KEYS]).toEqual(specFixture.stage_keys);
  });

  it('哪些键不是真实阶段，与表里的 pseudo_keys 一致', () => {
    // 经公开谓词断言私有的 `PSEUDO_KEYS`——它正是那份表的唯一消费者。
    // 两个方向都断：表里说是伪键的必须判伪键，表里没说的必须判真阶段。
    const pseudo = new Set(specFixture.pseudo_keys);
    for (const key of specFixture.stage_keys) {
      expect(isPseudoStage(key), `${key} 的伪键判定与表不一致`).toBe(pseudo.has(key));
    }
    // **集合相等而不是「在 stage_keys 上逐个核对」**：`PSEUDO_KEYS` 里多出一个**不是**
    // stage_keys 成员的值时，上面那个循环一次都走不到它（它根本不在被遍历的清单里），
    // 而这个多出来的键会让 `PUT /stage-configs` 收下一个前端根本不展示的键——正是
    // `stage_configs.rs` 那段注释警告的方向之一。故这里读**源文本**取出那个集合的字面量
    // （它私有、无枚举路径），与表比集合。同一种病不能用同一种药再治一遍：直接导出内部
    // 集合是为了测试放宽接口，读源文本则不必。
    const privateKeys = pseudoKeysFromSource();
    expect([...privateKeys].sort(), 'PSEUDO_KEYS 的成员集与表不一致').toEqual(
      [...pseudo].sort(),
    );
    // 认不出的输入一律不是伪键（防「谓词退化成恒真」）。
    expect(isPseudoStage('不是任何键')).toBe(false);
  });

  it('终态集与表里的 terminal_statuses 一致', () => {
    // `TERMINAL_STATUSES` 是模块私有的，经 `stewardshipFace` 断言——它是那份表的唯一
    // 消费者（终态任务不摆这颗钮），故强度与直接比表相同。
    //
    // TaskStatus 的**成员**在这里手写：那不是本条要钉的东西（决策 254 把 `types.ts` 的
    // 「形」的镜像押后，其中就包括 `TaskStatus` 联合本身）。本条钉的是「终态是哪几个」
    // 这条**规格**——前端多认一个终态、或少认一个，都会在这条上现形。
    const allStatuses: TaskStatus[] = [
      'queued',
      'waiting',
      'running',
      'pending',
      'done',
      'failed',
      'cancelled',
    ];
    const terminal = new Set(specFixture.terminal_statuses);
    for (const status of allStatuses) {
      // 未接线时 `stewardshipFace` 一律 null，故这里必须传 `true` 才测得到终态那一支。
      const face = stewardshipFace({ status, stewardship: null }, true);
      expect(face === null, `${status} 的终态判定与表不一致`).toBe(terminal.has(status));
    }
    // 非空守卫：接线为假时确实一律 null（免得上面那条因为「永远返回 null」而全绿）。
    expect(stewardshipFace({ status: 'running', stewardship: null }, false)).toBeNull();
  });

  it('pendingLabel 覆盖表里每一个 pending 种类（未覆盖即红）', () => {
    for (const kind of membersFixture.pending_kind) {
      const label = pendingLabel({ type: kind });
      expect(label, `pendingLabel 没覆盖 ${kind}，退回了原样`).not.toBe(kind);
    }
    // 认不出的种类原样返回——判据是「它 != 输入」，故上面那条只有在真覆盖时才绿。
    expect(pendingLabel({ type: UNKNOWN_KIND })).toBe(UNKNOWN_KIND);

    // `PENDING_KIND_MEMBERS` 与成员表 fixture 的一致性由 `enumMembersFixture.test.ts` 钉，
    // 这里只顺带确认上面遍历的那份清单就是前端认的那一份。
    expect([...PENDING_KIND_MEMBERS]).toEqual(membersFixture.pending_kind);
  });

  it('pendingLabel 的 user_decision 子类覆盖表里每一项', () => {
    for (const kind of specFixture.user_decision_context_kinds) {
      const label = pendingLabel({ type: 'user_decision', context: { kind } });
      expect(label, `pendingLabel 没覆盖 user_decision/${kind}`).not.toBe('等待决定');
    }
    // 没有 `context.kind`、以及认不出的，都退到通用那一行（决策 205 的同一口径）。
    expect(pendingLabel({ type: 'user_decision' })).toBe('等待决定');
    expect(pendingLabel({ type: 'user_decision', context: { kind: UNKNOWN_KIND } })).toBe(
      '等待决定',
    );
  });
});
