/**
 * @vitest-environment node
 *
 * 跨语言 golden fixture 的 **Node 侧那一半**（票 e2e-mock/01，决策 151 的 2026-09-13 修订）。
 *
 * 问题：playwright 的 mock（`frontend/e2e/harness.ts`）与 Rust 侧 `testkit` 的 mock 各自
 * 维护一份「OpenAI 兼容 SSE」的构造，两份之间**没有任何自动检查**——字段形状漂移时
 * Node 侧静默回「脚本已结束」文本（任务卡到超时），usage 漂移则完全静默。
 *
 * 修法：一份**提交进仓库**的 `tests/fixtures/e2e_mock_sse.json` 把三处钉在一起——
 * 本文件（Node 构造函数）、`crates/testkit/src/mock_llm.rs`（Rust mock）、
 * `crates/core/src/agent/providers/openai.rs`（消费它的适配器）。任一侧漂移即变红。
 *
 * 比较口径（与 Rust 侧一致，见 fixture 头注）：
 * - 字节层只钉 **SSE 帧**——`data: ` 前缀、空行分帧、`[DONE]` 终止；
 * - 字段层钉**解析后的结构**（JSON 键序不是契约：serde_json 按键序输出、JS 按插入序）；
 * - 工具调用 `id` 与 `function.arguments` 现场生成 / 由调用方字符串化，比对前归一
 *   （id → fixture 的固定值，arguments → 解析成对象）。
 *
 * **跑在 node 环境**（照 `lib/behavior-map.test.ts` 的先例）：默认的 jsdom 下
 * `import.meta.url` 是 http 形态、`fileURLToPath` 会抛——而本用例既要按 `import.meta.url`
 * 定位仓库根的 fixture，又要 import `e2e/harness.ts`（它顶部就做同一件事）。本用例不碰 DOM，
 * 故整个文件切到 node 环境是零代价的。
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { sseText, sseTool } from '../../e2e/harness';

/** fixture 定位走 `import.meta.url`（node 环境下是 file: 协议，见文件头注）。 */
const fixturePath = fileURLToPath(
  new URL('../../../tests/fixtures/e2e_mock_sse.json', import.meta.url),
);

interface FixtureCase {
  name: string;
  producer: 'sseTool' | 'sseText';
  /** producer 的入参（与 `frontend/e2e/scripts.ts` 的 Step 形态一致）。 */
  call: { name?: string; arguments?: unknown; text?: string };
  /** 期望的 SSE 文本（键序不参与比对，帧与字段参与）。 */
  sse: string;
}

interface Fixture {
  usage: { prompt_tokens: number; completion_tokens: number };
  cases: FixtureCase[];
}

const fixture: Fixture = JSON.parse(readFileSync(fixturePath, 'utf8')) as Fixture;
const FIXTURE_CALL_ID = 'call_fixture_1';

/**
 * 把一段 SSE 文本拆成**可解析的字节契约**：逐事件的载荷（`[DONE]` 留作字符串）。
 * 帧层断言（`data: ` 前缀 / 空行分帧）在拆的过程中一并钉住。
 */
function frames(sse: string): unknown[] {
  expect(sse.startsWith('data: '), `缺 \`data: \` 前缀：${sse}`).toBe(true);
  expect(sse.endsWith('\n\n'), `结尾须是空行：${sse}`).toBe(true);
  return sse
    .split('\n\n')
    .filter((event) => event !== '')
    .map((event) => {
      expect(event.startsWith('data: '), `事件缺 \`data: \` 前缀：${event}`).toBe(true);
      const payload = event.slice('data: '.length);
      if (payload === '[DONE]') return payload;
      const value = JSON.parse(payload) as {
        choices?: Array<{
          delta?: {
            tool_calls?: Array<{ id?: string; function?: { arguments?: string } }>;
          };
        }>;
        usage?: unknown;
      };
      // 归一：id 现场生成；arguments 是「谁产出按谁键序」的不透明串，下游按 JSON 解析它
      for (const call of value.choices?.[0]?.delta?.tool_calls ?? []) {
        if (call.id !== undefined) call.id = FIXTURE_CALL_ID;
        const raw = call.function?.arguments;
        if (typeof raw === 'string') {
          try {
            call.function!.arguments = JSON.parse(raw) as never;
          } catch {
            // 不是 JSON 就原样比（本 fixture 里不会出现）
          }
        }
      }
      return value;
    });
}

describe('跨语言 golden fixture（票 e2e-mock/01）', () => {
  it('fixture 覆盖三种步骤形态', () => {
    expect(fixture.cases.map((c) => c.name).sort()).toEqual(['submit', 'text', 'tool_call']);
  });

  for (const c of fixture.cases) {
    it(`harness 构造函数产出与 fixture 逐字段一致：${c.name}`, () => {
      const actual =
        c.producer === 'sseTool'
          ? sseTool(c.call.name as string, c.call.arguments)
          : sseText(c.call.text as string);

      // ① 帧 + 逐字段（id / arguments 已归一）
      expect(frames(actual)).toEqual(frames(c.sse));
      // ② fixture 那一份本身也要满足同一份帧契约（fixture 写坏了同样红）
      expect(frames(c.sse)).toHaveLength(3); // chunk / usage / [DONE]
    });
  }

  it('usage 常量与 fixture 同源（漂移曾是完全静默的那一处）', () => {
    const usage = (
      frames(sseText('x'))[1] as { usage: { prompt_tokens: number; completion_tokens: number } }
    ).usage;
    expect(usage).toEqual(fixture.usage);
  });

  it('工具调用的字段位置与适配器消费路径一致（choices[0].delta.tool_calls[]）', () => {
    const toolCase = fixture.cases.find((c) => c.producer === 'sseTool') as FixtureCase;
    const parsed = frames(sseTool(toolCase.call.name as string, toolCase.call.arguments))[0] as {
      choices: Array<{
        index: number;
        finish_reason: string;
        delta: { role: string; tool_calls: Array<Record<string, unknown>> };
      }>;
    };
    const call = parsed.choices[0].delta.tool_calls[0];
    expect(parsed.choices[0].index).toBe(0);
    expect(parsed.choices[0].delta.role).toBe('assistant');
    expect(parsed.choices[0].finish_reason).toBe('tool_calls');
    expect(call).toMatchObject({ index: 0, id: FIXTURE_CALL_ID, type: 'function' });
    expect((call.function as { name: string }).name).toBe(toolCase.call.name);
    expect((call.function as { arguments: unknown }).arguments).toEqual(toolCase.call.arguments);
  });
});
