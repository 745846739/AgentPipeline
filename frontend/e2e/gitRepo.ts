/**
 * 技能来源的**离线 git fixture**（E2E ⑫⑬，决策 194 / 票 03）。
 *
 * 两件东西合在一个模块里：
 *   1. 一个**裸仓**（`git init --bare` + 一个推它的工作树）——仓里有若干技能目录，覆盖两种
 *      深度形态（`skills/x` 与 `plugins/{插件}/skills/x`）与「技能目录带子树兄弟文件」；
 *   2. 一个 **git smart HTTP** 服务，只实现 `upload-pack` 两条路由，把请求转给系统 `git`。
 *
 * 于是 E2E 走的是**真 libgit2 路径**：真 HTTP 传输、`depth(1)` shallow、按裸 SHA 取 commit、
 * 对象哈希由 libgit2 在 fetch 时本地校验。整套装置不打真网络（回环明文 http 由票 01 的
 * `AGENTPIPELINE_MARKET_GIT_BASE` 接缝放行，与决策 177③ 的回环例外同源）。
 *
 * ## 两条路由与它们的 `Content-Type`
 *
 * ```
 * GET  /{owner}/{repo}.git/info/refs?service=git-upload-pack
 *      → git upload-pack --stateless-rpc --advertise-refs <裸仓>
 *        Content-Type: application/x-git-upload-pack-advertisement
 *        响应体 = 服务公告行 + flush-pkt + 广告
 * POST /{owner}/{repo}.git/git-upload-pack
 *      → 请求体喂 `git upload-pack --stateless-rpc <裸仓>` 的 stdin
 *        Content-Type: application/x-git-upload-pack-result
 * ```
 *
 * **dumb HTTP 不行**：libgit2 硬校验响应的 `Content-Type` 必须是
 * `application/x-git-upload-pack-advertisement`，而 `git update-server-info` +
 * 普通静态服务器回的是 `application/octet-stream`，实测被拒（`invalid content-type`）。
 * 故这里不走静态文件那条路，而是现起 `upload-pack` 转发——与真 GitHub 同一形态。
 *
 * ## 服务公告行必须**带 pkt-line 长度前缀**
 *
 * 响应体开头是 `001e# service=git-upload-pack\n` 加 `0000`（flush-pkt），而不是裸的
 * `# service=git-upload-pack\n`：git 的 smart HTTP 协议里这一行**本身是一个 pkt-line**，
 * 前 4 个十六进制字节是它的长度（0x001e = 4 + 26），libgit2 的 `smart_pkt.c` 正是按
 * 「前 4 字节当长度、取回来一看首字符是 `#` 就归类为注释并跳过」来解析它的。裸文本会让
 * 那 4 个字节落到 `# se` 上而对不出十六进制长度。真 `git http-backend` 发的就是这个带
 * 前缀的形态。
 *
 * ## 裸仓必须开 `uploadpack.allowAnySHA1InWant`
 *
 * 本机 git 默认是 `false`，而不开时按裸 SHA 取**任何** commit 都会在**客户端**就被 libgit2
 * 拦下（`cannot fetch a specific object from the remote repository`，发生在发出任何 pack
 * 请求之前），现象是「还没联网就失败了」。真 GitHub 的广告里
 * `allow-tip-sha1-in-want` / `allow-reachable-sha1-in-want` 两个能力位都在，故 fixture 开
 * 这个位是为了**与 GitHub 行为一致**，不是为了迁就 fixture。
 */

import { execFileSync, spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import { tmpdir } from 'node:os';
import * as path from 'node:path';

/** 一个技能目录（`dir` 是仓内相对路径，不含末尾的 `SKILL.md`）。 */
export interface GitSkill {
  /** 技能目录，如 `skills/grilling` 或 `plugins/agent-kit/skills/tdd`。 */
  dir: string;
  /** frontmatter 的 `description`（列表里显示它；空串表示不写这一行）。 */
  description: string;
  /**
   * `SKILL.md` 的正文（frontmatter 之后的部分）。**逐行写，行号即可预判**——
   * `frontmatter` 占 1–4 行、第 5 行空行，故 `body[0]` 落在第 6 行。
   */
  body: string[];
  /** 技能目录下的子树兄弟文件（键是相对技能目录的路径）——钉「递归读子树」。 */
  siblings?: Record<string, string>;
}

export interface GitRepoFixture {
  /** `http://127.0.0.1:<port>`（无尾斜杠）——填进 `AGENTPIPELINE_MARKET_GIT_BASE`。 */
  base: string;
  owner: string;
  repo: string;
  /** 当前 tip 的 40 位 hex。 */
  tip(): string;
  /** 收到的请求行（`METHOD /path`）。断言「没点添加之前零请求」用。 */
  requests: string[];
  /**
   * 请求体里出现过的 `want <40 位 SHA>`。
   *
   * 这是「看到的 = 装到的」唯一可观测的证据：列表钉住某个 commit 之后，即使远端 tip 前进，
   * 后续的 fetch 也只能 want 那个旧 commit。
   */
  wants: string[];
  /** 再追加一个 commit（返回它的 40 位 hex），用来把 tip 往前推。 */
  commit(message: string, files?: Record<string, string>): string;
  close(): Promise<void>;
}

/** 服务公告行的 pkt-line：`0x001e` = 4 + `"# service=git-upload-pack\n"` 的 26 字节。 */
const SERVICE_PREFIX = '001e# service=git-upload-pack\n0000';

/** 同步跑一条 git 命令（fixture 搭建期用；失败时把 stderr 带出来）。 */
function git(cwd: string, args: string[]): string {
  return execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

/** 异步跑 `git upload-pack`：`input` 喂 stdin，返回 stdout 的全部字节。 */
function runUploadPack(input: Buffer | null, args: string[], cwd: string): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const child = spawn('git', args, { cwd, stdio: ['pipe', 'pipe', 'pipe'] });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    child.stdout.on('data', (chunk: Buffer) => out.push(chunk));
    child.stderr.on('data', (chunk: Buffer) => err.push(chunk));
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolve(Buffer.concat(out));
      else {
        reject(
          new Error(
            `git ${args.join(' ')} -> ${code}：${Buffer.concat(err).toString('utf8')}`,
          ),
        );
      }
    });
    child.stdin.end(input ?? Buffer.alloc(0));
  });
}

/** 按本系统的单根布局生成 `SKILL.md`：frontmatter 的 `name` **必须**等于目录名。 */
function skillMd(dir: string, skill: GitSkill): string {
  const name = path.posix.basename(dir);
  return [
    '---',
    `name: ${name}`,
    `description: ${skill.description}`,
    '---',
    '',
    ...skill.body,
    '',
  ].join('\n');
}

function writeFile(root: string, rel: string, contents: string): void {
  const abs = path.join(root, rel);
  mkdirSync(path.dirname(abs), { recursive: true });
  writeFileSync(abs, contents);
}

/**
 * 起一套装置：一个裸仓 + 一个只服务它的 smart HTTP 服务。
 *
 * 服务只认 `opts.owner/opts.repo` 这一个仓，别的路径一律 404（但仍然记请求日志）——
 * 于是「界面偷偷去 fetch 了推荐名单里的某个仓」会在日志里留下一条 404，而不是静默通过。
 */
export async function startGitRepo(opts: {
  owner: string;
  repo: string;
  skills: GitSkill[];
  /** 仓根上的额外文件（键是仓内相对路径）。 */
  files?: Record<string, string>;
}): Promise<GitRepoFixture> {
  const tmpRoot = mkdtempSync(path.join(tmpdir(), 'agentpipeline-gitrepo-'));
  const work = path.join(tmpRoot, 'work');
  const bare = path.join(tmpRoot, 'bare.git');
  mkdirSync(work, { recursive: true });

  // 裸仓是「远端」，工作树推给它——这样推进 tip 就是一次真 push（与用户推进自己的仓同形态）。
  // **`-b main` 不能省**：后端用 `default_branch()` 发现 tip（票 01 第 4 条），而默认初始化的
  // 裸仓 HEAD 指向 `refs/heads/master`（一个不存在的分支），推上去的 `main` 就永远走不到。
  git(tmpRoot, ['init', '--bare', '-b', 'main', bare]);
  git(work, ['init', '-b', 'main']);
  git(work, ['config', 'user.name', 'e2e']);
  git(work, ['config', 'user.email', 'e2e@localhost']);
  git(work, ['remote', 'add', 'origin', bare]);

  const commit = (message: string, files: Record<string, string> = {}): string => {
    for (const [rel, contents] of Object.entries(files)) writeFile(work, rel, contents);
    git(work, ['add', '-A']);
    git(work, ['commit', '-m', message]);
    git(work, ['push', 'origin', 'main']);
    return git(work, ['rev-parse', 'HEAD']).trim();
  };

  for (const skill of opts.skills) {
    writeFile(work, `${skill.dir}/SKILL.md`, skillMd(skill.dir, skill));
    for (const [rel, contents] of Object.entries(skill.siblings ?? {})) {
      writeFile(work, `${skill.dir}/${rel}`, contents);
    }
  }
  for (const [rel, contents] of Object.entries(opts.files ?? {})) writeFile(work, rel, contents);
  commit('chore: 初始技能仓');

  // 与真 GitHub 一致：允许按裸 SHA 取未被广告的 commit（理由见模块头）
  git(bare, ['config', 'uploadpack.allowAnySHA1InWant', 'true']);

  const requests: string[] = [];
  const wants: string[] = [];

  const server: Server = createServer((req, res) => {
    const url = req.url ?? '/';
    requests.push(`${req.method ?? 'GET'} ${url}`);
    const pathname = url.split('?')[0] ?? url;
    const route = /^\/([^/]+)\/([^/]+)\.git\/(info\/refs|git-upload-pack)$/.exec(pathname);

    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => {
      const body = Buffer.concat(chunks);
      // 请求体是 pkt-line 流（`0032want <sha> multi_ack …`），把 want 的 SHA 记下来
      for (const m of body.toString('utf8').matchAll(/want ([0-9a-f]{40})/g)) {
        if (m[1]) wants.push(m[1]);
      }

      const notFound = () => {
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      };
      if (!route || route[1] !== opts.owner || route[2] !== opts.repo) return notFound();

      const service = new URL(url, 'http://127.0.0.1').searchParams.get('service');
      if (route[3] === 'info/refs' && req.method === 'GET' && service === 'git-upload-pack') {
        void runUploadPack(
          null,
          ['upload-pack', '--stateless-rpc', '--advertise-refs', bare],
          tmpRoot,
        )
          .then((out) => {
            res.writeHead(200, {
              'content-type': 'application/x-git-upload-pack-advertisement',
              'cache-control': 'no-cache',
            });
            res.end(Buffer.concat([Buffer.from(SERVICE_PREFIX, 'utf8'), out]));
          })
          .catch((err: Error) => {
            res.writeHead(500, { 'content-type': 'text/plain' });
            res.end(err.message);
          });
        return;
      }
      if (route[3] === 'git-upload-pack' && req.method === 'POST') {
        void runUploadPack(body, ['upload-pack', '--stateless-rpc', bare], tmpRoot)
          .then((out) => {
            res.writeHead(200, {
              'content-type': 'application/x-git-upload-pack-result',
              'cache-control': 'no-cache',
            });
            res.end(out);
          })
          .catch((err: Error) => {
            res.writeHead(500, { 'content-type': 'text/plain' });
            res.end(err.message);
          });
        return;
      }
      notFound();
    });
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;

  return {
    base: `http://127.0.0.1:${port}`,
    owner: opts.owner,
    repo: opts.repo,
    tip: () => git(work, ['rev-parse', 'HEAD']).trim(),
    requests,
    wants,
    commit,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => {
          rmSync(tmpRoot, { recursive: true, force: true });
          resolve();
        });
      }),
  };
}
