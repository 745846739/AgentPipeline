import { describe, expect, it } from 'vitest';
import { parseUnifiedDiff } from './diff';

/**
 * unified diff 解析（决策 416 B 的前端半边）。
 *
 * 后端 `merge-proposal.diff` 的头部可能带一块 `[autosave]` 说明（合入前自动落提交，
 * 决策 416 B）——它**必须**被解析器整块无视：进不了任何文件的行表，也不许污染
 * `files_changed` / `insertions` / `deletions`。若哪天有人把这段注记改成以
 * `diff --git ` 或 `--- ` 开头（或把它追加到尾部），这条用例就该红。
 */

const NOTE = [
  '# [autosave] 合入前自动落提交（决策 416 B）',
  '',
  '工作区里有 2 处未提交的已跟踪改动；rebase 前管线把它们落成提交 `abc1234`（带 `[autosave]` 标记）。',
  '它们本来就是本任务的产出，因此下面这份 diff 里也有它们——标记是它们与 agent 手写',
  '提交之间唯一的区别。',
  '',
  '未提交清单：',
  '  - `src/lib.ts`',
  '  - `tests/a.test.ts`',
  '',
].join('\n');

const RAW = [
  'diff --git a/src/lib.ts b/src/lib.ts',
  'index 111..222 100644',
  '--- a/src/lib.ts',
  '+++ b/src/lib.ts',
  '@@ -1,3 +1,3 @@',
  ' export function a() {',
  '-  return 1;',
  '+  return 2;',
  ' }',
  'diff --git a/tests/a.test.ts b/tests/a.test.ts',
  'new file mode 100644',
  '--- /dev/null',
  '+++ b/tests/a.test.ts',
  '@@ -0,0 +1,2 @@',
  '+export function t() {}',
  '+export function u() {}',
  '',
].join('\n');

describe('parseUnifiedDiff', () => {
  it('无注记时的基准统计', () => {
    const { stats, files } = parseUnifiedDiff(RAW);
    expect(stats.files_changed).toBe(2);
    expect(stats.insertions).toBe(3);
    expect(stats.deletions).toBe(1);
    expect(files.map((f) => f.path)).toEqual(['src/lib.ts', 'tests/a.test.ts']);
  });

  it('头部 [autosave] 注记整块不可见：统计与文件表与无注记时逐字段相同', () => {
    const bare = parseUnifiedDiff(RAW);
    const noted = parseUnifiedDiff(NOTE + RAW);
    expect(noted.stats).toEqual(bare.stats);
    expect(noted.files.map((f) => f.path)).toEqual(bare.files.map((f) => f.path));
    // 注记的行一行都不许漏进文件行表
    const texts = noted.files.flatMap((f) => f.lines.map((l) => l.text));
    expect(texts.some((t) => t.includes('[autosave]'))).toBe(false);
    expect(texts.some((t) => t.includes('未提交清单'))).toBe(false);
  });

  it('注记中的 - 开头行不计 deletions（头部未开文件时整块跳过）', () => {
    // 注记块里以 `-` 单独成行的说明行（形如 `- 某文件`）若被当成 diff 行，
    // deletions 会凭空多出来——这条钉住「头部跳过」的语义。
    const hostile = ['# 说明', '- 会被当成删除的行', ''].join('\n') + RAW;
    const { stats } = parseUnifiedDiff(hostile);
    expect(stats.deletions).toBe(1);
    expect(stats.insertions).toBe(3);
  });
});
