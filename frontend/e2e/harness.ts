/**
 * 前端 E2E harness（票 18，决策 144 / 150 / 151）。
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
 *   5. Vite dev server：`VITE_API_PROXY_TARGET` 指向回读到的后端端口，
 *      前端保持同源相对 API base（决策 153④），HTTP + SSE 全部经 Vite 代理。
 *
 * 用例结束（含失败）时 `stop()` 回收全部子进程与临时目录。
 */

import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import { createServer, type Server } from 'node:http';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

import type { NodeScript, Step } from './scripts';

const here = fileURLToPath(new URL('.', import.meta.url));
const frontendDir = path.resolve(here, '..');
const repoRoot = path.resolve(frontendDir, '..');

/** 与 testkit FakeAgent 相同的替换边界：只换 LLM 流。 */
export interface StartOptions {
  /** 按 `(stage, node)` / `pseudo:*` 组织的脚本队列；每项是「轮」的序列。 */
  script: NodeScript;
  /** 任务标题（断言用）。 */
  title?: string;
}

export interface App {
  /** 真 axum 后端 origin（回读端口的真值）。 */
  apiBase: string;
  /** Vite 前端 origin（浏览器入口）。 */
  webBase: string;
  /** 播种出的任务 id。 */
  taskId: string;
  /** 直接读后端（断言轮询用，不走 UI）。 */
  getTask(): Promise<Record<string, unknown>>;
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
];

function routeKey(system: string): string | null {
  for (const [marker, key] of PERSONA_ROUTES) {
    if (system.includes(marker)) return key;
  }
  return null;
}

function sseTool(name: string, args: unknown): string {
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
    `data: ${JSON.stringify(chunk)}\n\n` +
    `data: ${JSON.stringify({ usage: { prompt_tokens: 10, completion_tokens: 5 } })}\n\n` +
    'data: [DONE]\n\n'
  );
}

function sseText(text: string): string {
  const chunk = {
    choices: [{ index: 0, delta: { role: 'assistant', content: text }, finish_reason: 'stop' }],
  };
  return (
    `data: ${JSON.stringify(chunk)}\n\n` +
    `data: ${JSON.stringify({ usage: { prompt_tokens: 10, completion_tokens: 5 } })}\n\n` +
    'data: [DONE]\n\n'
  );
}

/** 启动 mock LLM，返回 `{ url, close }`。 */
async function startMockLlm(script: NodeScript): Promise<{
  url: string;
  close: () => Promise<void>;
}> {
  const state: MockState = {
    rounds: new Map(Object.entries(script).map(([k, v]) => [k, v.map((r) => [...r])])),
    round: new Map(),
    step: new Map(),
    calls: 0,
  };
  const server: Server = createServer((req, res) => {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      state.calls += 1;
      const payload = JSON.parse(body || '{}') as {
        messages?: Array<{ role?: string }>;
      };
      const messages = payload.messages ?? [];
      const system = (messages[0] as { content?: string } | undefined)?.content ?? '';
      const key = routeKey(system);
      // 新节点运行：请求只有 system + user（openai.rs 的 build_body 在上述两条之后再追加
      // 历史 messages）。每见到一次新一轮，轮指针 +1（首次 → 0），步骤指针归零。
      if (key && messages.length <= 2) {
        state.round.set(key, (state.round.get(key) ?? -1) + 1);
        state.step.set(key, 0);
      }
      const queue = key ? state.rounds.get(key) : undefined;
      const roundIdx = key ? (state.round.get(key) ?? 0) : 0;
      const stepIdx = key ? (state.step.get(key) ?? 0) : 0;
      const round = queue?.[roundIdx];
      const step = round?.[stepIdx];
      if (key) state.step.set(key, stepIdx + 1);
      let out: string;
      if (!step) {
        // 脚本耗尽 → 无 tool_call 的收尾响应，agent loop 自然结束（与 FakeAgent 同语义）
        out = sseText('（脚本已结束）');
      } else if (step.kind === 'tool') {
        out = sseTool(step.name, step.args);
      } else if (step.kind === 'submit') {
        out = sseTool('submit_metadata', step.value);
      } else {
        out = sseText(step.text);
      }
      res.writeHead(200, {
        'content-type': 'text/event-stream',
        'cache-control': 'no-cache',
        connection: 'close',
      });
      res.end(out);
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
  };
}

async function freePort(): Promise<number> {
  return new Promise<number>((resolve, reject) => {
    const s = net.createServer();
    s.on('error', reject);
    s.listen(0, '127.0.0.1', () => {
      const addr = s.address();
      const port = typeof addr === 'object' && addr ? addr.port : 0;
      s.close(() => resolve(port));
    });
  });
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

/** fixture 仓库：main + 一次提交，无 language 标记文件（闸门命令为 `true`）。 */
function makeFixtureRepo(dir: string): void {
  const git = (args: string[]) =>
    execFileSync('git', args, { cwd: dir, stdio: 'pipe', env: process.env });
  git(['init', '-b', 'main']);
  git(['config', 'user.name', 'e2e']);
  git(['config', 'user.email', 'e2e@localhost']);
  writeFileSync(path.join(dir, 'README.md'), '# e2e fixture\n');
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

/** 启动整套装置（mock LLM + 真后端 + Vite），并播种 provider / 项目 / 任务。 */
export async function startApp(opts: StartOptions): Promise<App> {
  const title = opts.title ?? 'E2E 任务';
  const tmpRoot = mkdtempSync(path.join(tmpdir(), 'agentpipeline-e2e-'));
  const homeDir = path.join(tmpRoot, 'home');
  const repoDir = path.join(tmpRoot, 'repo');
  const children: ChildProcess[] = [];

  // 1) 临时 home + 配置（1s tick 让准入在秒级发生；resume 冷却 0 便于用例驱动）
  mkdirSync(homeDir, { recursive: true });
  mkdirSync(repoDir, { recursive: true });
  writeFileSync(
    path.join(homeDir, 'config.toml'),
    '[pipeline]\ntick_interval_sec = 1\npending_resume_cooldown_sec = 0\n',
  );

  // 2) fixture 仓库
  makeFixtureRepo(repoDir);

  // 3) mock LLM
  const mock = await startMockLlm(opts.script);

  const cleanup = async () => {
    for (const child of children.reverse()) {
      if (child.exitCode === null) {
        child.kill('SIGINT');
        await new Promise((r) => setTimeout(r, 300));
        if (child.exitCode === null) child.kill('SIGKILL');
      }
    }
    await mock.close().catch(() => undefined);
    rmSync(tmpRoot, { recursive: true, force: true });
  };

  try {
    // 4) 真后端：--port 0 + 回读就绪行端口
    const bin = resolveBinary();
    const backend = spawn(bin, ['serve', '--port', '0'], {
      cwd: repoRoot,
      env: { ...process.env, AGENTPIPELINE_HOME: homeDir, RUST_LOG: 'warn' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    children.push(backend);
    const apiPort = await readReadyPort(backend, 30_000);
    const apiBase = `http://127.0.0.1:${apiPort}`;
    await waitForHttp(`${apiBase}/metrics`, 20_000);

    // 5) 播种：provider 指向 mock → 项目指向 fixture 仓库 → 任务
    await postJson(apiBase, '/providers', {
      vendor: 'openai',
      model: 'mock',
      context_window: 8000,
      base_url: mock.url,
      api_key: 'sk-test',
      enabled: true,
    });
    const project = await postJson<{ project: { id: string } }>(apiBase, '/projects', {
      name: 'e2e',
      local_path: repoDir,
    });
    const task = await postJson<{ task: { id: string } }>(apiBase, '/tasks', {
      project_id: project.project.id,
      title,
    });
    const taskId = task.task.id;

    // 6) Vite dev server：API 透传到回读到的后端端口（决策 153④）
    const vitePort = await freePort();
    const viteBin = path.join(frontendDir, 'node_modules', '.bin', 'vite');
    const vite = spawn(viteBin, ['--host', '127.0.0.1', '--port', String(vitePort), '--strictPort'], {
      cwd: frontendDir,
      env: { ...process.env, VITE_API_PROXY_TARGET: apiBase },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    children.push(vite);
    const webBase = `http://127.0.0.1:${vitePort}`;
    await waitForHttp(`${webBase}/`, 30_000);

    return {
      apiBase,
      webBase,
      taskId,
      getTask: () => getJson<Record<string, unknown>>(apiBase, `/tasks/${taskId}`),
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
  const deadline = Date.now() + timeoutMs;
  let last = '';
  while (Date.now() < deadline) {
    const task = await fetchTask(app);
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
