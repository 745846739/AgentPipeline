/**
 * 前端 E2E harness（票 18，决策 144 / 150 / 151；主流程票 01）。
 *
 * 一条用例 = 一套**独占**的进程与目录：
 *   1. 临时 home（`AGENTPIPELINE_HOME`）——绝不碰真实 `~/.agentpipeline`；
 *   2. 临时 git fixture 仓库（无 language 标记文件 → `test_framework = None`
 *      → 系统闸门命令为 `true`，让 fixture 不必是可构建工程）；
 *   3. Node 手写 mock LLM（OpenAI 兼容 SSE，按 system prompt 的 persona 反查
 *      `(stage, node)` 消费脚本）——**只替换 LLM 响应流**，工具 / git / 命令全真跑
 *      （与 testkit FakeAgent 同一替换边界，决策 148）；
 *   4. **真** `agent-pipeline` 二进制：`serve --port 0`，从 stdout 确定性就绪行
 *      `AGENTPIPELINE_READY port=<n>` **回读内核分配的真实端口**（决策 153⑤，
 *      不再有「先探测再释放」的竞争窗口）；
 *   5. **浏览器入口就是后端 origin 本身**（主流程票 01）：后端经决策 155 同源托管
 *      **编译期内嵌的** `frontend/dist`，因此页面加载的是用户实际会加载的那份产物
 *      ——真实 JS bundle、真实 CI 态、零代理（不再起 Vite dev server）。
 *
 * 用例结束（含失败）时 `stop()` 回收全部子进程与临时目录。
 */

import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import { createServer, type Server } from 'node:http';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Page } from '@playwright/test';

import type { NodeScript, Step } from './scripts';
import { FOREMAN } from './scripts';

const here = fileURLToPath(new URL('.', import.meta.url));
const frontendDir = path.resolve(here, '..');
const repoRoot = path.resolve(frontendDir, '..');

/** 与 testkit FakeAgent 相同的替换边界：只换 LLM 流。 */
export interface StartOptions {
  /** 按 `(stage, node)` / `pseudo:*` 组织的脚本队列；每项是「轮」的序列。 */
  script: NodeScript;
  /** 任务标题（断言用）。 */
  title?: string;
  /**
   * **配错的 provider**（主流程票 03）：播种时指向一个**恒定返回 HTTP 错误**的 mock，
   * 任务必然失败挂起——用于验证「配错可理解、可恢复」。`fixProvider()` 之后走脚本流。
   */
  badProvider?: { status: number; body: string };
  /**
   * **空 home 启动**（主流程票 05）：不播种 provider / 项目 / 任务，
   * 用例全程走 UI 创建三件套。此时 `taskId` 为空串，`fixProvider()` 不可用。
   *
   * 与 {@link providerOnly} 的区别：那个留一个可用 provider（值班长答得上话），
   * 这个连 provider 都不播（首启真的一无所有）。
   */
  seedless?: boolean;
  /**
   * **只播 provider，不建项目与任务**（票 04「空看板也能对话」）：
   * 值班长在任何 home 状态都要答得上话，而它要能答话就至少得有一个可用 provider
   * ——这正是首启用户的真实处境（刚填完模型与密钥，还没接入项目）。
   * 此时 `taskId` 为空串，`getTask()` 不可用。
   */
  providerOnly?: boolean;
  /** 评审模式（主流程票 06）：`human` 时任务停在 `pending(human_review)`。缺省 agent。 */
  reviewMode?: 'agent' | 'human';
  /**
   * **同项目并发任务**（主流程票 09）：在同 home / 同项目内再播种的任务，
   * 各带**按标题路由**的独立脚本——mock 用 user prompt 里的任务标题区分任务，
   * 各任务消费各自的脚本轮（默认 `script` 归属主任务与未匹配标题的任务）。
   */
  additionalTasks?: Array<{ title: string; script: NodeScript }>;
  /**
   * **技能来源是 GitHub 仓**（票 03 / 决策 194）：把用例的仓与后端的 git 接缝一起接上。
   *
   * 传了它就会：① 在临时 home 的 `config.toml` 里写 `[market] github_repos`（可由
   * {@link StartOptions.market.seedConfig} 关掉）；② 给后端子进程加
   * `AGENTPIPELINE_MARKET_GIT_BASE`（票 01 的测试接缝，取值**只允许 https 或回环 http**）。
   */
  market?: {
    /** `owner/repo` 的前一段。 */
    owner: string;
    /** 仓名。 */
    repo: string;
    /**
     * GitHub 的**基础地址**（`AGENTPIPELINE_MARKET_GIT_BASE`，默认 `https://github.com`）。
     * E2E 填 `gitRepo.ts` 起的离线 smart HTTP 的 `base`（`http://127.0.0.1:<port>`）。
     */
    gitBase?: string;
    /**
     * 是否把 `owner/repo` 写进 `config.toml` 的 `[market] github_repos`。**默认 true**
     * ——那是「界面与配置两级关系」的起点（E2E ⑫）。置 false 时 home 里没有 `[market]`，
     * 用例必须自己经过界面把仓加进去（E2E ⑬ 的「添加 = 放行」）。
     */
    seedConfig?: boolean;
  };
  /**
   * 追加进 `config.toml` 的 `[pipeline]` 段的行（票 04 的值守轮用例用）：
   * 值守的节拍旋钮（`watch_debounce_sec` 等，`crates/core/src/config.rs` 的 Settings）
   * 都挂在这一段下。缺省不写任何行——其余用例吃全局默认（值守 60s 去抖在秒级用例里
   * 等于「不醒」，这正是此前所有用例都不用管值守轮的原因）。
   */
  pipelineConfig?: string[];
  /**
   * 写进 `config.toml` 的 `[server]` 段的行（决策 334）。目前只有 `public_base_url`
   * 用得上：「手机访问」页在**有公网入口**时的形态（画码、不给改绑钮）与缺省的回环形态
   * 是两条不同的路径，而它只有把真后端配起来才跑得到——组件层只能喂假读数，
   * 字段名在后端与前端之间漂了它照样绿。
   */
  serverConfig?: string[];
}

export interface App {
  /** 真 axum 后端 origin（回读端口的真值）。 */
  apiBase: string;
  /** 浏览器入口 origin。主流程票 01 起**与 `apiBase` 同值**：后端同源托管内嵌产物。 */
  webBase: string;
  /** fixture 仓库路径（用例断言合入产物时用 `git -C <repoDir> ...`）。 */
  repoDir: string;
  /**
   * 临时 home（`AGENTPIPELINE_HOME`）。
   *
   * 用例直接读磁盘时用——值班长的域就是它，提议执行之后文件**真的**落在这里，
   * 「按下确认钮不是只改了一行状态」由这一条取证（票 03 / 04）。
   */
  homeDir: string;
  /** 播种出的任务 id（`seedless` 模式下为空串）。 */
  taskId: string;
  /** 全部播种任务 id（主任务在前，`additionalTasks` 次之；主流程票 09）。 */
  taskIds: string[];
  /**
   * mock LLM 的 base_url（主流程票 05）：UI 建 provider 时填进表单，
   * 让 UI 创建的任务也走脚本 mock。
   */
  mockUrl: string;
  /** 直接读后端（断言轮询用，不走 UI）。 */
  getTask(): Promise<Record<string, unknown>>;
  /**
   * mock 记录到的**新节点运行**请求（system + user 一条一记录，主流程票 05）。
   * 断言「用户填的描述真的进了 prompt」用。
   */
  prompts(): Array<{ system: string; user: string }>;
  /** 后端进程 stderr/stdout 的环形缓冲（诊断与日志断言用，主流程票 06/07）。 */
  backendLogs(): string[];
  /**
   * **事后换值班长的脚本轮**（testkit `set_script` 的等价物）——供「下一轮要用前一轮产出的
   * id」这类用例（修复那条链：`repair_id` 由后端生成）。
   */
  setForemanRounds(rounds: Step[][]): void;
  /**
   * 修复 provider（主流程票 03）：把 base_url 从坏 mock 切回脚本 mock，
   * 模拟「用户到设置页改好了 base_url / api_key」。仅在 `badProvider` 下有意义。
   */
  fixProvider(): Promise<void>;
  stop(): Promise<void>;
}

/**
 * 每节点一个「轮」指针：一轮 = 该节点一次运行内的步骤序列。
 * `messages.length <= 2`（system + user，openai.rs 的 body 组装）表示**新一轮节点运行**
 * ——pending → resume 后重入同一节点即消费下一轮，等价于 testkit 注记里的
 * 「多轮行为用 set_script 分轮投喂」。
 */
interface MockState {
  rounds: Map<string, Step[][]>;
  round: Map<string, number>;
  step: Map<string, number>;
  calls: number;
}

/** persona 首句 → 节点键（顺序敏感：先长后短）。 */
const PERSONA_ROUTES: Array<[string, string]> = [
  ['你是架构设计的信息充分性检查 agent', 'architect-design.validate_input'],
  ['你是架构设计 agent', 'architect-design.execute'],
  ['你是架构设计的产出质量检查 agent', 'architect-design.validate_output'],
  ['你是开发方案的输入充分性检查 agent', 'develop-design.validate_input'],
  ['你是开发方案 agent', 'develop-design.execute'],
  ['你是开发方案的产出质量检查 agent', 'develop-design.validate_output'],
  ['你是测试设计的输入充分性检查 agent', 'test-design.validate_input'],
  ['你是业务测试用例设计 agent', 'test-design.execute'],
  ['你是测试设计的产出质量检查 agent', 'test-design.validate_output'],
  ['你是开发 agent', 'develop.execute'],
  ['你是代码评审 agent', 'review.execute'],
  ['你是测试 agent', 'test.execute'],
  ['你是设计语义冲突比对 agent', 'pseudo:conflict_check'],
  ['你是独立复核 agent', 'pseudo:validator_cross_check'],
  ['你是项目分析 agent', 'pseudo:project_analysis'],
  // 值班长（票 01 的对讲台）：人格首句见 crates/core/src/pipeline/foreman.rs::FOREMAN_PERSONA。
  // 它是个**任务无关**的调用方，没有 stage/node，故按人格路由到独立的 foreman 槽。
  ['你是夜班车间的值班长', 'foreman'],
];

function routeKey(system: string): string | null {
  for (const [marker, key] of PERSONA_ROUTES) {
    if (system.includes(marker)) return key;
  }
  return null;
}

/**
 * 一个工具调用步的 SSE 字节（tool call / submit 两种步都走它）。
 *
 * **导出供 vitest 侧断言**（票 e2e-mock/01）：`frontend/src/lib/e2e-mock-fixture.test.ts`
 * 用它产出字节，再与提交进仓库的 `tests/fixtures/e2e_mock_sse.json` 逐字段比对；
 * Rust 侧（testkit 的 `sse_tool` / 适配器的 `parse_chunk`）指向同一份 fixture。
 * 工具调用 `id` 现场生成，比对前按 fixture 的归一约定替换。
 *
 * 该用例跑在 **node 环境**（`@vitest-environment node`，照 `lib/behavior-map.test.ts` 的先例）：
 * jsdom 环境下本文件顶部的 `fileURLToPath(new URL('.', import.meta.url))` 会抛
 * （`import.meta.url` 是 http 形态），node 环境下 `import.meta.url` 才是 file: 协议。
 */
export function sseTool(name: string, args: unknown, reasoning?: string): string {
  const chunk = {
    choices: [
      {
        index: 0,
        delta: {
          role: 'assistant',
          tool_calls: [
            {
              index: 0,
              id: `call_${Date.now()}_${Math.floor(Math.random() * 1e6)}`,
              type: 'function',
              function: { name, arguments: JSON.stringify(args) },
            },
          ],
        },
        finish_reason: 'tool_calls',
      },
    ],
  };
  return (
    (reasoning ? sseReasoning(reasoning) : '') +
    `data: ${JSON.stringify(chunk)}\n\n` +
    `data: ${JSON.stringify({ usage: { prompt_tokens: 10, completion_tokens: 5 } })}\n\n` +
    'data: [DONE]\n\n'
  );
}

/** 一个文本步的 SSE 字节。导出理由同 {@link sseTool}（票 e2e-mock/01）。 */
export function sseText(text: string, reasoning?: string): string {
  const chunk = {
    choices: [{ index: 0, delta: { role: 'assistant', content: text }, finish_reason: 'stop' }],
  };
  return (
    (reasoning ? sseReasoning(reasoning) : '') +
    `data: ${JSON.stringify(chunk)}\n\n` +
    `data: ${JSON.stringify({ usage: { prompt_tokens: 10, completion_tokens: 5 } })}\n\n` +
    'data: [DONE]\n\n'
  );
}

/**
 * **推理增量**（决策 244 的 `reasoning` 声道）：一个只带 `reasoning_content` 的 delta。
 *
 * 字段名两个都收（`openai.rs::parse_chunk`），这里用 DeepSeek / 多数兼容网关那一个。
 * 它存在的理由：段序（决策 273）要断言的第一件事就是「想」与「查」的先后——没有这一支，
 * e2e 里的一轮只有工具与收口，**顺序对不对看不出来**（工具的次序本来就是脚本给的）。
 */
function sseReasoning(text: string): string {
  const chunk = {
    choices: [{ index: 0, delta: { role: 'assistant', reasoning_content: text } }],
  };
  return `data: ${JSON.stringify(chunk)}\n\n`;
}

/**
 * **一段流中间的增量**（`drip` 步的装置，决策 260）：只有一个 content delta，
 * **没有 `usage`、没有 `[DONE]`**——那两样都在另一半里。
 *
 * 为什么不复用 {@link sseText}：它带 `[DONE]`，而适配器一读到那个标记就认为流结束了
 * （`openai.rs::parse_chunk`）——两截都用它的话，后半截永远不会到。
 */
function sseDelta(text: string): string {
  const chunk = { choices: [{ index: 0, delta: { role: 'assistant', content: text } }] };
  return `data: ${JSON.stringify(chunk)}\n\n`;
}

/** 启动 mock LLM，返回 `{ url, close, prompts, bindTask }`。
 *
 * `extra`（主流程票 09）：按任务路由的附加脚本。**优先按任务 id 匹配**——只有
 * architect-design 的 user prompt 带「任务标题：{title}」，develop / review / test
 * 的 prompt 只有路径（`{dev_doc_path}` / `{changed_files}` …）与 system prompt 的
 * 「## 工作目录」段，两处都含**任务 id**（`tasks/{id}` / `worktrees/{id}`）。
 * 只按标题路由会让第二个任务在中后段落回默认脚本（票 09 实测：develop.execute
 * 拿到「脚本已结束」文本 → `retry_exhausted`）。`bindTask(id, title)` 在播种后绑定，
 * 标题匹配保留作为兜底。
 *
 * 轮/步指针按 `owner|node` 分键，任务之间互不串扰。
 */
async function startMockLlm(
  script: NodeScript,
  extra: Array<{ title: string; script: NodeScript }>,
): Promise<{
  url: string;
  close: () => Promise<void>;
  prompts: () => Array<{ system: string; user: string }>;
  bindTask: (taskId: string, title: string) => void;
  setRounds: (key: string, rounds: Step[][]) => void;
}> {
  const state: MockState = {
    rounds: new Map(Object.entries(script).map(([k, v]) => [k, v.map((r) => [...r])])),
    round: new Map(),
    step: new Map(),
    calls: 0,
  };
  const extraScripts = extra.map((e) => ({
    title: e.title,
    rounds: new Map(Object.entries(e.script).map(([k, v]) => [k, v.map((r) => [...r])])),
  }));
  /** 播种后绑定的 `task_id → 标题`（用于 prompt 里的 id 反查归属）。 */
  const boundIds = new Map<string, string>();
  // 「新节点运行」的请求记录（主流程票 05）：断言用户填的描述真的进了 prompt
  const promptLog: Array<{ system: string; user: string }> = [];
  const server: Server = createServer((req, res) => {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      state.calls += 1;
      const payload = JSON.parse(body || '{}') as {
        messages?: Array<{ role?: string; content?: string }>;
      };
      const messages = payload.messages ?? [];
      const system = (messages[0] as { content?: string } | undefined)?.content ?? '';
      const user = (messages[1] as { content?: string } | undefined)?.content ?? '';
      const key = routeKey(system);
      // 任务归属（主流程票 09）：先按 prompt 里出现的任务 id 反查（全阶段可用），
      // 再退回标题匹配（architect 阶段，兼容未 bindTask 的调用）；都不中走默认脚本。
      const byId = [...boundIds.entries()].find(
        ([id]) => system.includes(id) || user.includes(id),
      )?.[1];
      const owner = byId ?? extraScripts.find((e) => user.includes(e.title))?.title ?? '';
      const effRounds =
        extraScripts.find((e) => e.title === owner)?.rounds ?? state.rounds;
      // `key === null`（认不出的请求）时取空串：下面每一处使用都由 `key` 把关，
      // 这个值不会被读到；写成 string 是为了让类型检查看得懂这件事（svelte-check 会
      // 顺着 vitest 的 import 检查本文件——票 e2e-mock/01）。
      const nodeKey = key === null ? '' : owner ? `${owner}\u0000${key}` : key;
      // 新节点运行：**最后一条是 user**（与值班长同一条判据，2026-09-26 统一）。
      //
      // 从前节点用「`messages.length <= 2`」（system + user，openai.rs 的 build_body 在
      // 那两条之后追加历史 messages），那条在**转录被续接**的几处形状上不再成立：
      //   · pending → resume 重入（决策 279：补充输入 = 转录末尾的 user 轮）
      //   · 失败重试（决策 278：转录原样保留 + 一条错误 turn = 末尾的 user 轮）
      // 两种重入**都该**是新一轮，而旧判据把它们看成同一次工具往返——轮指针不前进、
      // 脚本被耗尽、节点判「没等到 submit_metadata」（pending-resume ① 与 review-branch
      // ②③ 就是这么红的：`msgs=6 new=false` 一条条打出来，轮号一直停在 0）。
      //
      // 同一次工具往返以 **tool** 收尾（assistant 发起、tool 结果回来），故两种情形仍分得开；
      // 值班长本来就是这个判据（它每轮请求 = 历史对话 + 末尾一轮快照合并的 user 轮，条数
      // 不固定，「≤2」对它永不成立）——两条路从此同源。
      const lastRole = messages[messages.length - 1]?.role ?? '';
      const isNewRound = lastRole === 'user';
      if (key && isNewRound) {
        state.round.set(nodeKey, (state.round.get(nodeKey) ?? -1) + 1);
        state.step.set(nodeKey, 0);
        promptLog.push({ system, user });
      }
      const queue = key ? effRounds.get(key) : undefined;
      const roundIdx = key ? (state.round.get(nodeKey) ?? 0) : 0;
      const stepIdx = key ? (state.step.get(nodeKey) ?? 0) : 0;
      const round = queue?.[roundIdx];
      const step = round?.[stepIdx];
      if (key) state.step.set(nodeKey, stepIdx + 1);
      let out: string;
      if (!step) {
        // 脚本耗尽 → 无 tool_call 的收尾响应，agent loop 自然结束（与 FakeAgent 同语义）
        out = sseText('（脚本已结束）');
      } else if (step.kind === 'tool') {
        out = sseTool(step.name, step.args, step.reasoning);
      } else if (step.kind === 'submit') {
        out = sseTool('submit_metadata', step.value);
      } else if (step.kind === 'drip') {
        // 分两截滴：走下面的 `drip` 那一支（`out` 用不到，给个空串让类型收敛）
        out = '';
      } else if (step.kind === 'think') {
        // 分几截滴推理：同上（走下面的 `think` 那一支）
        out = '';
      } else {
        out = sseText(step.text, step.reasoning);
      }
      // **拖住再回**（决策 260 的用例装置）：一轮「还在跑」因此有一段可观测的窗口，
      // 「跑着的时候刷新页面」才抢得到。字节仍是上面那一份产出的，只有**何时**发出去不同。
      const delayMs = step && step.kind === 'text' ? (step.delayMs ?? 0) : 0;
      // 客户端**中途走了**是常态，不是装置故障：停钮（决策 294）与关页都会把这一条
      // 连接掐掉，而这里可能还在 `setTimeout` 里等着写。写一个已销毁的 socket 会抛，
      // 抛在定时器里就是未捕获异常——那会把整个 mock 进程带走，后续用例跟着全红。
      const write = (fn: () => void) => {
        try {
          fn();
        } catch {
          /* 客户端已经走了：这一份字节没有收件人，不是错误 */
        }
      };
      const send = () =>
        write(() => {
          res.writeHead(200, {
            'content-type': 'text/event-stream',
            'cache-control': 'no-cache',
            connection: 'close',
          });
          res.end(out);
        });
      // **分两截滴**（同上）：前半截先真写出去，隔 `gapMs` 再补后半截并收线。刷新之后的
      // 那一段增量因此是**真的在刷新之后到达的**——旧闸门会把它丢掉，用例于是有牙齿。
      if (step && step.kind === 'drip') {
        write(() => {
          res.writeHead(200, {
            'content-type': 'text/event-stream',
            'cache-control': 'no-cache',
            connection: 'close',
          });
          res.write(sseDelta(step.parts[0] ?? ''));
        });
        step.parts.slice(1).forEach((part, i) => {
          const last = i === step.parts.length - 2;
          setTimeout(
            () =>
              write(() => {
                if (last) {
                  res.end(
                    sseDelta(part) +
                      `data: ${JSON.stringify({ usage: { prompt_tokens: 10, completion_tokens: 5 } })}\n\n` +
                      'data: [DONE]\n\n',
                  );
                } else {
                  res.write(sseDelta(part));
                }
              }),
            step.gapMs * (i + 1),
          );
        });
        return;
      }
      // **分几截滴推理**（`think` 步，决策 301 的 ticker 装置）：与上面那一支同一个形状，
      // 只把每截的内容换成**推理声道**；最后一截与收口正文一起收线——收线之前那一段里
      // 「推理正在攒」这个状态是真的，ticker 因此可观测。
      if (step && step.kind === 'think') {
        const parts = step.parts.length > 0 ? step.parts : [''];
        write(() => {
          res.writeHead(200, {
            'content-type': 'text/event-stream',
            'cache-control': 'no-cache',
            connection: 'close',
          });
          res.write(sseReasoning(parts[0]));
        });
        parts.slice(1).forEach((part, i) => {
          const last = i === parts.length - 2;
          setTimeout(
            () =>
              write(() => {
                if (last) res.end(sseReasoning(part) + sseText(step.text));
                else res.write(sseReasoning(part));
              }),
            step.gapMs * (i + 1),
          );
        });
        return;
      }
      if (delayMs > 0) setTimeout(send, delayMs);
      else send();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const addr = server.address();
  const port = typeof addr === 'object' && addr ? addr.port : 0;
  return {
    url: `http://127.0.0.1:${port}`,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
    prompts: () => promptLog.map((p) => ({ ...p })),
    bindTask: (taskId, taskTitle) => {
      boundIds.set(taskId, taskTitle);
    },
    /**
     * **事后换脚本**（testkit `set_script` 的等价物）。
     *
     * 为什么需要它：有些值只有**跑起来之后**才知道（后端生成的 `project_id`、修复 id），
     * 而脚本在 `startApp` 之前就写死了——本文件那一列路由注释记的正是这个口子缺着的代价
     * （「mock 没有事后注入脚本的口子」）。补上它之后，多轮之间就能按前一轮的产出改下一轮。
     *
     * 轮指针归零：下一次「新轮」从新脚本的第 0 轮开始（与 testkit 逐轮重设同一语义）。
     */
    setRounds: (key: string, rounds: Step[][]) => {
      state.rounds.set(
        key,
        rounds.map((r) => [...r]),
      );
      state.round.set(key, -1); // 下一次新轮 → 索引 0（服务那一刻会 +1）
      state.step.set(key, 0);
    },
  };
}

/**
 * 启动一个**恒定返回 HTTP 错误**的 mock（主流程票 03）。
 *
 * 模拟「用户配错了密钥 / 地址」：provider 指向它时每个节点必然失败，
 * 任务挂 `pending(retry_exhausted)` 且 message 是中文可操作提示。
 */
async function startRejectingMock(
  status: number,
  body: string,
): Promise<{ url: string; close: () => Promise<void> }> {
  const server: Server = createServer((req, res) => {
    // OpenAI 兼容端点：无论路径，一律回错（错误不依赖请求体）
    void req.resume();
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const addr = server.address();
  const port = typeof addr === 'object' && addr ? addr.port : 0;
  return {
    url: `http://127.0.0.1:${port}`,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
  };
}

async function waitForHttp(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastErr = 'timeout';
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.ok || res.status < 500) return;
      lastErr = `HTTP ${res.status}`;
    } catch (err) {
      lastErr = (err as Error).message;
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`等待 ${url} 就绪超时：${lastErr}`);
}

async function postJson<T>(base: string, route: string, body: unknown): Promise<T> {
  const res = await fetch(`${base}${route}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`POST ${route} -> ${res.status}: ${text}`);
  return JSON.parse(text) as T;
}

async function getJson<T>(base: string, route: string): Promise<T> {
  const res = await fetch(`${base}${route}`);
  const text = await res.text();
  if (!res.ok) throw new Error(`GET ${route} -> ${res.status}: ${text}`);
  return JSON.parse(text) as T;
}

/**
 * fixture 仓库：真实可构建的 **Node 工程**（主流程票 02）。
 *
 * 为什么不再是无语言标记的空仓库：`detect_language` 返回 `None` 时
 * `test_command_for(None)` 退化为 **`true`**——闸门变成空操作，而闸门是用户主流程里
 * 必然经过的一环（跑测试 → 失败分流 lint/test → 复检）。带 `package.json` 后
 * 探测为 `node` → `npm` → 闸门真跑 `npm test --silent`。
 *
 * 选 Node 而非 Rust 的理由：`npm test` 走零依赖的 `node run-tests.js`，实测约 0.4s，
 * 不影响 e2e 时长；Rust fixture 的 `cargo test` 编译耗时会让每条用例不可接受地变慢，
 * 且本仓库自身的 `cargo build` 已在跑 cargo。
 *
 * 初始 `src/lib.js` 故意是**失败占位**（任务语义上就是「实现 add」），develop 阶段
 * 写入正解后闸门转绿——这样闸门失败与通过两条路径都能被真实验证。
 */
function makeFixtureRepo(dir: string): void {
  const git = (args: string[]) =>
    execFileSync('git', args, { cwd: dir, stdio: 'pipe', env: process.env });
  git(['init', '-b', 'main']);
  git(['config', 'user.name', 'e2e']);
  git(['config', 'user.email', 'e2e@localhost']);

  mkdirSync(path.join(dir, 'src'), { recursive: true });
  writeFileSync(
    path.join(dir, 'package.json'),
    `${JSON.stringify(
      { name: 'e2e-fixture', private: true, scripts: { test: 'node run-tests.js' } },
      null,
      2,
    )}\n`,
  );
  // 闸门命令：零依赖，退出码即结论。
  writeFileSync(
    path.join(dir, 'run-tests.js'),
    "const { add } = require('./src/lib.js');\n" +
      "if (add(1, 2) !== 3) { console.error('FAIL: add(1,2) !== 3'); process.exit(1); }\n" +
      "console.log('PASS: add');\n",
  );
  // 初始占位（未实现）——develop 阶段会被替换为真实实现。
  writeFileSync(
    path.join(dir, 'src/lib.js'),
    "function add() { throw new Error('not implemented'); }\nmodule.exports = { add };\n",
  );

  git(['add', '-A']);
  git(['commit', '-m', 'chore: init']);
}

/** 从子进程 stdout 读就绪行 `AGENTPIPELINE_READY port=<n>`（决策 153⑤）。 */
function readReadyPort(child: ChildProcess, timeoutMs: number): Promise<number> {
  return new Promise<number>((resolve, reject) => {
    let buffer = '';
    let stderr = '';
    const timer = setTimeout(() => reject(new Error(`等待就绪行超时；stderr=${stderr}`)), timeoutMs);
    child.stdout?.on('data', (chunk: Buffer) => {
      buffer += chunk.toString();
      let idx: number;
      while ((idx = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, idx).trim();
        buffer = buffer.slice(idx + 1);
        const prefix = 'AGENTPIPELINE_READY port=';
        if (line.startsWith(prefix)) {
          const port = Number.parseInt(line.slice(prefix.length), 10);
          if (Number.isFinite(port) && port > 0) {
            clearTimeout(timer);
            resolve(port);
            return;
          }
        }
      }
    });
    child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error(`后端在打印就绪行前退出（code=${code}）；stderr=${stderr}`));
    });
  });
}

function resolveBinary(): string {
  if (process.env.AGENTPIPELINE_E2E_BIN) return process.env.AGENTPIPELINE_E2E_BIN;
  return path.join(repoRoot, 'target', 'debug', 'agent-pipeline');
}

/**
 * 前置守卫（主流程票 01）：确认被测二进制**内嵌了前端产物**。
 *
 * 决策 155 的 build.rs 在 `frontend/dist` 缺失时生成**空资产表**——此时 `GET /` 退化为
 * 「前端未构建」提示页，浏览器拿不到真实 bundle。若不拦，用例会在「页面能打开」的假象下
 * 跑到断言超时才失败，且失败信息直指业务而非根因。此处提前给出可操作的报错。
 */
async function assertEmbeddedBundle(apiBase: string): Promise<void> {
  const res = await fetch(`${apiBase}/`);
  const body = await res.text();
  if (!body.includes('/assets/')) {
    throw new Error(
      '被测二进制未内嵌前端产物（GET / 返回「前端未构建」提示页）。\n' +
        '先构建产物再跑 e2e：`cd frontend && npm run build && cd .. && cargo build`。\n' +
        '若要指向其他二进制，设置 AGENTPIPELINE_E2E_BIN。',
    );
  }
}

/** 启动整套装置（mock LLM + 真后端；浏览器入口即后端同源 origin）。 */
export async function startApp(opts: StartOptions): Promise<App> {
  const title = opts.title ?? 'E2E 任务';
  const tmpRoot = mkdtempSync(path.join(tmpdir(), 'agentpipeline-e2e-'));
  const homeDir = path.join(tmpRoot, 'home');
  const repoDir = path.join(tmpRoot, 'repo');
  const children: ChildProcess[] = [];

  // 1) 临时 home + 配置（1s tick 让准入在秒级发生；resume 冷却 0 便于用例驱动）
  mkdirSync(homeDir, { recursive: true });
  mkdirSync(repoDir, { recursive: true });
  const configLines = ['[pipeline]', 'tick_interval_sec = 1', 'pending_resume_cooldown_sec = 0'];
  if (opts.pipelineConfig) configLines.push(...opts.pipelineConfig);
  // 技能来源的**声明式默认**那一级（`config.toml` 的 `[market] github_repos`，决策 194）。
  // 界面上的那一份盖过它；清掉界面那份就回到这里——两级关系由 E2E ⑫ 钉住。
  if (opts.market && opts.market.seedConfig !== false) {
    configLines.push('[market]', `github_repos = ["${opts.market.owner}/${opts.market.repo}"]`);
  }
  // `[server]`（决策 334）：公网入口那一套。写在最后，与上面的段各自成节。
  if (opts.serverConfig?.length) {
    configLines.push('[server]', ...opts.serverConfig);
  }
  writeFileSync(path.join(homeDir, 'config.toml'), `${configLines.join('\n')}\n`);

  // 2) fixture 仓库
  makeFixtureRepo(repoDir);

  // 3) mock LLM（脚本流）+ 可选的「配错 provider」mock（主流程票 03）
  const mock = await startMockLlm(opts.script, opts.additionalTasks ?? []);
  const bad = opts.badProvider
    ? await startRejectingMock(opts.badProvider.status, opts.badProvider.body)
    : null;

  const cleanup = async () => {
    for (const child of children.reverse()) {
      if (child.exitCode === null) {
        child.kill('SIGINT');
        await new Promise((r) => setTimeout(r, 300));
        if (child.exitCode === null) child.kill('SIGKILL');
      }
    }
    await mock.close().catch(() => undefined);
    await bad?.close().catch(() => undefined);
    rmSync(tmpRoot, { recursive: true, force: true });
  };

  try {
    // 4) 真后端：--port 0 + 回读就绪行端口
    const bin = resolveBinary();
    const backend = spawn(bin, ['serve', '--port', '0'], {
      cwd: repoRoot,
      env: {
        ...process.env,
        AGENTPIPELINE_HOME: homeDir,
        // 票 01 的测试接缝：GitHub 的基础地址（默认 https://github.com，只允许 https 或回环 http）
        ...(opts.market?.gitBase ? { AGENTPIPELINE_MARKET_GIT_BASE: opts.market.gitBase } : {}),
        RUST_LOG: process.env.E2E_RUST_LOG ?? 'warn',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    children.push(backend);
    // 后端 stderr/stdout 环形缓冲（票 06/07）：失败诊断与执行器 warn 的现场证据
    const backendLog: string[] = [];
    const collectLog = (chunk: Buffer) => {
      for (const line of chunk.toString('utf8').split('\n')) {
        if (!line.trim()) continue;
        backendLog.push(line);
      }
      if (backendLog.length > 800) backendLog.splice(0, backendLog.length - 800);
    };
    backend.stdout?.on('data', collectLog);
    backend.stderr?.on('data', collectLog);
    const apiPort = await readReadyPort(backend, 30_000);
    const apiBase = `http://127.0.0.1:${apiPort}`;
    await waitForHttp(`${apiBase}/metrics`, 20_000);
    // 主流程票 01：浏览器要加载的是内嵌产物，先确认它真的在（否则报错而非假绿）。
    await assertEmbeddedBundle(apiBase);

    // 5) 播种：provider 指向 mock → 项目指向 fixture 仓库 → 任务
    //    `badProvider` 时 provider 指向坏 mock（任务必然失败）——fixProvider() 后切回脚本流。
    //    `seedless`（主流程票 05）跳过播种：三件套由用例走 UI 创建，此处只留空 home。
    //    `providerOnly`（票 04）只播 provider：空看板但值班长答得上话（首启态）。
    let providerId: string | null = null;
    let taskId = '';
    const taskIds: string[] = [];
    if (!opts.seedless) {
      const providerBase = bad ? bad.url : mock.url;
      const seeded = await postJson<{ provider: { id: string } }>(apiBase, '/providers', {
        vendor: 'openai',
        model: 'mock',
        context_window: 8000,
        base_url: providerBase,
        api_key: 'sk-test',
        enabled: true,
      });
      providerId = seeded.provider.id;
    }
    if (!opts.seedless && !opts.providerOnly) {
      const project = await postJson<{ project: { id: string } }>(apiBase, '/projects', {
        name: 'e2e',
        local_path: repoDir,
      });
      const task = await postJson<{ task: { id: string } }>(apiBase, '/tasks', {
        project_id: project.project.id,
        title,
        ...(opts.reviewMode ? { review_mode: opts.reviewMode } : {}),
      });
      taskId = task.task.id;
      taskIds.push(taskId);
      mock.bindTask(taskId, title);
      // 主流程票 09：同项目并发任务（同一 mock，脚本按任务 id / 标题路由）
      for (const extra of opts.additionalTasks ?? []) {
        const t = await postJson<{ task: { id: string } }>(apiBase, '/tasks', {
          project_id: project.project.id,
          title: extra.title,
        });
        taskIds.push(t.task.id);
        mock.bindTask(t.task.id, extra.title);
      }
    }

    // 6) 浏览器入口 = 后端 origin：内嵌 dist 经 axum 同源托管（决策 155），
    //    API base 保持同源相对路径（决策 153④），零代理零 CORS。
    return {
      apiBase,
      webBase: apiBase,
      repoDir,
      homeDir,
      taskId,
      taskIds,
      mockUrl: mock.url,
      getTask: () => getJson<Record<string, unknown>>(apiBase, `/tasks/${taskId}`),
      prompts: () => mock.prompts(),
      backendLogs: () => [...backendLog],
      setForemanRounds: (rounds) => mock.setRounds(FOREMAN, rounds),
      fixProvider: async () => {
        if (!bad || !providerId)
          throw new Error('fixProvider() 仅在 badProvider 模式下有意义');
        await fetch(`${apiBase}/providers/${providerId}`, {
          method: 'PATCH',
          headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
          body: JSON.stringify({ base_url: mock.url }),
        });
      },
      stop: cleanup,
    };
  } catch (err) {
    await cleanup();
    throw err;
  }
}

export interface TaskSnapshot {
  status: string;
  pending_type: string | null;
}

/**
 * `GET /tasks/{id}` 返回 `{ task, cursors, allowed_actions, blocks, depends_on }` 包装体
 * （§12.4.1）——轮询谓词消费的是其中的 `task`，此处解包，避免调用方读到 undefined。
 */
export async function fetchTask(app: App): Promise<Record<string, unknown>> {
  const body = await app.getTask();
  const inner = body.task;
  if (!inner || typeof inner !== 'object') {
    throw new Error(`任务详情响应缺少 task 字段：${JSON.stringify(body)}`);
  }
  return inner as Record<string, unknown>;
}

/** 轮询后端直到任务满足谓词（绕开 UI 的确定性等待）。 */
export async function waitForTask(
  app: App,
  predicate: (task: Record<string, unknown>) => boolean,
  description: string,
  timeoutMs = 120_000,
): Promise<void> {
  return waitForTaskById(app.taskId, app, predicate, description, timeoutMs);
}

/** 同上，但显式指定任务 id（`seedless` 模式下任务由 UI 创建，主流程票 05）。 */
export async function waitForTaskById(
  taskId: string,
  app: App,
  predicate: (task: Record<string, unknown>) => boolean,
  description: string,
  timeoutMs = 120_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let last = '';
  while (Date.now() < deadline) {
    const res = await fetch(`${app.apiBase}/tasks/${taskId}`);
    if (!res.ok) throw new Error(`GET /tasks/${taskId} -> ${res.status}`);
    const body = (await res.json()) as { task: Record<string, unknown> };
    const task = body.task;
    last = JSON.stringify(task);
    if (predicate(task)) return;
    if (task.status === 'failed' || task.status === 'cancelled') {
      throw new Error(`任务进入终态 ${String(task.status)}，未达成「${description}」：${last}`);
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`等待「${description}」超时：${last}`);
}

export function pendingTypeOf(task: Record<string, unknown>): string | null {
  const reason = task.pending_reason as { type?: string } | null | undefined;
  return reason?.type ?? null;
}

/** 任务命令行（主流程票 02：断言闸门**真的执行了**测试命令）。 */
export interface CommandRow {
  id: number;
  command: string;
  source: string;
  cwd?: string;
  exit_code?: number | null;
}

/** 读任务的命令记录（`GET /tasks/{id}/commands`）。 */
export async function fetchCommands(app: App): Promise<CommandRow[]> {
  const body = await getJson<{ commands: CommandRow[] }>(app.apiBase, `/tasks/${app.taskId}/commands`);
  return body.commands ?? [];
}

/**
 * 断言闸门**真的跑了**测试命令（主流程票 02）。
 *
 * 为什么必须断言：fixture 若退化成无语言标记仓库，闸门命令是 `true`——任务照样能到 done，
 * 「流程走完」的断言会全绿，而闸门从未执行。这里检查命令记录里存在由**系统**发起的
 * 真实测试命令（`npm test` / fixture 的 `test` 脚本），把「闸门被短路」变成红。
 */
export async function expectGateReallyRan(app: App): Promise<void> {
  const commands = await fetchCommands(app);
  const system = commands.filter((c) => c.source === 'system');
  const gate = system.filter((c) => /\bnpm\b|run-tests|node\b/.test(c.command));
  if (gate.length === 0) {
    throw new Error(
      '闸门未真正执行测试命令——疑似 fixture 退化（无语言标记 → 命令为 `true`）。\n' +
        `系统命令共 ${system.length} 条：${system.map((c) => c.command).join(' | ') || '（无）'}`,
    );
  }
}

/**
 * 任务状态快照（主流程票 02 的闸门失败断言用）。
 *
 * 注意 `gate_failures` **不在这里**：它属于 merge 阶段产出（`MergeResult`，见
 * `types.rs:725`），不在任务详情里；develop 闸门失败时还没进 merge，故那侧的
 * 失败证据由「命令记录里测试命令的退出码非 0」+「游标 attempts / 阶段落点」承担。
 */
export async function fetchGateState(app: App): Promise<{
  status: string;
  pendingType: string | null;
}> {
  const task = await fetchTask(app);
  return {
    status: String(task.status ?? ''),
    pendingType: pendingTypeOf(task),
  };
}

/** 命令记录里是否有**退出码非 0** 的测试命令（闸门真失败的证据）。 */
export async function findFailedGateCommand(app: App): Promise<CommandRow | null> {
  const commands = await fetchCommands(app);
  return (
    commands.find(
      (c) => c.source === 'system' && c.exit_code !== null && c.exit_code !== 0 &&
        /\bnpm\b|run-tests|node\b/.test(c.command),
    ) ?? null
  );
}

/**
 * 真实产物的完整性守卫（主流程票 01）：挂到 page 上收集「内嵌 bundle 跑不起来」的信号。
 *
 * 为什么必须有：仅断言 `page.goto` 成功对**白屏**同样会绿——HTML 能返回、JS 没执行时，
 * 页面就是一块空白。这里把静态资源非 2xx、模块解析失败、未捕获异常都变成待断言的集合，
 * 由 {@link expectBundleHealthy} 在页面稳定后统一裁决。
 */
export interface BundleGuard {
  /** 静态资源加载失败（`/assets/*` 非 2xx）或页面级错误。 */
  problems: string[];
}

/** 在 `page.goto` 之前调用；返回的 guard 交给 {@link expectBundleHealthy} 裁决。 */
export function watchBundle(page: Page): BundleGuard {
  const guard: BundleGuard = { problems: [] };
  page.on('response', (res) => {
    const url = res.url();
    // `/assets/*` 是 Vite 产物；`/fonts/*` 是主题六自托管的像素字体子集
    // （决策 169）——字体 404 会让主题静默回退成系统 monospace，必须一并堵住。
    // `/sw.js` 是 service worker 的脚本（pwa-webpush 票 03）：它 404 时
    // `register()` 只会静默 reject，页面照常——那条线只有在推送那一节才看得见。
    if ((url.includes('/assets/') || url.includes('/fonts/') || url.includes('/sw.js')) && !res.ok()) {
      guard.problems.push(`静态资源非 2xx：${res.status()} ${url}`);
    }
  });
  page.on('pageerror', (err) => {
    guard.problems.push(`未捕获的页面错误：${err.message}`);
  });
  page.on('console', (msg) => {
    if (msg.type() !== 'error') return;
    const text = msg.text();
    // 只收「产物加载/执行」类错误：外部字体 preconnect、无关的第三方告警不算产物问题。
    if (/(\/assets\/|imported module|MIME type|Failed to load module|SyntaxError)/i.test(text)) {
      guard.problems.push(`console.error：${text}`);
    }
  });
  return guard;
}

/** 断言内嵌产物在浏览器里健康加载（无 404 / 无未捕获错误）。 */
export function expectBundleHealthy(guard: BundleGuard): void {
  if (guard.problems.length > 0) {
    throw new Error(`内嵌前端产物在浏览器中加载异常：\n- ${guard.problems.join('\n- ')}`);
  }
}

/**
 * 等页面把内嵌产物加载执行完（主流程票 01）。
 *
 * **不能用 `networkidle`**：任务详情与看板都常驻 SSE 流（`/tasks/{id}/stream`），
 * 连接永不空闲，`networkidle` 必然超时。`load` 才是正确事件——`type="module"` 脚本是
 * deferred 的，`load` 会等它们取回并执行完。之后再给一个很短的窗口收尾 console 错误，
 * 使白屏类失败在业务断言之前就落地（1s 级报根因，而非 60s 超时）。
 */
export async function settleBundle(page: Page, guard: BundleGuard): Promise<void> {
  await page.waitForLoadState('load');
  await page.waitForTimeout(300);
  expectBundleHealthy(guard);
}

/**
 * 展开现场页签里所有还收着的轮（决策 366：轮默认收起，正文要点一下才进视口）。
 *
 * **为什么读 `details.open` 属性而不是用 `:not([open])` 选择器**：展开态是 Svelte 写的
 * DOM 属性，选择器那一路要赌它同时反射成属性——赌输的表现是「点过的又被点到」，而这里
 * 一次点错只是白点一下，不会静默漏掉一个轮。节点不因翻转而重建（Svelte 只改 `open`），
 * 所以先取总数再按下标点，下标不会漂。
 *
 * **`summary` 取 `.first()`**：轮里还嵌着 prompt / 思考 / 工具三层折叠，它们各自也有
 * `summary`——文档序里轮自己那一条在最前（它是折叠块的第一个孩子），取首个即它。
 */
export async function expandSceneTurns(page: Page): Promise<void> {
  const folds = page.locator('article.turn details.turnfold');
  const total = await folds.count();
  for (let i = 0; i < total; i += 1) {
    const fold = folds.nth(i);
    if (await fold.evaluate((el) => (el as HTMLDetailsElement).open)) continue;
    await fold.locator('summary').first().click();
  }
}
