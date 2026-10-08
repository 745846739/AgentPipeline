# 01: 值班长的修复提交落在部署检出上，把下一次部署堵死

**What happened（2026-10-08 的一次真事故，部署期间发现）**：

- `deploy.sh` 在 106 上走 `git pull --ff-only`；而 `/opt/AgentPipeline` 的 `main` 当时
  **diverged**（`ahead 1, behind 6`）——本地多出一条 `ba77558`
  （author `agentpipeline-foreman`，`2026-10-08 03:01:32Z`，subject 以 `[repair]` 开头：
  值班长修复 `01M4CEFYE8XBZ3R98TKDFY3ZCV`，21 文件 / +897 −102）。
- 于是 `git pull` 连续 8 次 `fatal: Not possible to fast-forward, aborting`，
  deploy run 失败（`37723306534` 的第一趟），**每次部署都会同样失败**直到有人清掉它。
- 那条提交**从未推到 origin**，也**没有构建进正在跑的二进制**（服务与二进制都是更早那一次
  构建的产物）——它在检出里躺着，只起了一个作用：堵住部署。

**为什么会这样（根因，不是运维手滑）**：

1. 值班长的**修复轮**（决策 210③④）在一个**项目仓**上拉 worktree、跑闸门、落一个带
   `[repair]` 标记的提交，然后由人按「合入」把它并进**项目仓的当前分支**。
2. 106 上「AgentPipeline」这个项目注册的 `local_path` 就是 **`/opt/AgentPipeline`
   ——部署检出本身**。于是部署检出同时是两样东西：被部署的产物源（要求 `origin/main` 的
   ff）与一个**可被代理写入的项目仓**（允许在 main 上产生本地提交）。
3. 两者对 `main` 的要求互相矛盾：前者要求「只能 ff」，后者允许「本地前进」。
   一次修复就把这个矛盾炸出来，而**失败面是部署**（`deploy.sh` 只报 git 拉取失败，
   与「为什么 main 上有本地提交」隔着两层）。

**当时的处置（非破坏，作为先例）**：

- 保住那条提交的名分：106 上 `git branch repair/01M4CEFYE8XBZ3R98TKDFY3ZCV ba77558`，
  `git bundle` 带回本机并 **push 到 origin 的同名分支**（未并进 main）。
- 106 的 `main` `reset --hard origin/main`——**没有动任何任务 worktree**
  （`/root/.agentpipeline/worktrees/*` 与 `kanban/*` 分支一字未改）。
- 重跑 deploy → 成功。

**待裁决（三条出路，各自的代价如实记）**：

1. **修复轮跑在独立克隆上**：值班长的项目仓指向一份**只读/独享的克隆**
   （例如 `/root/.agentpipeline/projects/<id>`），`/opt/AgentPipeline` 只做部署
   （永远只 ff）。代价：多一份磁盘与一次 clone；修复产物要经 PR/补丁回流。
2. **部署检出改成「只 ff」并在拉取失败时自查本地提交**：`deploy.sh` 在
   `git pull --ff-only` 失败时打印 `git log origin/main..HEAD` 并**拒绝**静默继续。
   代价：仍然要人来清；但失败信息从「拉不下来」变成「这三条本地提交挡住了」。
3. **允许部署检出被修复**：`deploy.sh` 改 `git pull --rebase`/自动 stash。
   代价：**不推荐**——把「未经 CI 的本地提交」自动并进部署，闸门（决策 330）失去意义。

**验收（任一方向落地时）**：

- 用例/脚本层：`deploy.sh` 在 diverged 检出上**报出挡路的那几条提交**（而不是只报
  「能否 ff」）；或 `[repair]` 提交的落点是独立克隆（静态断言：`local_path != /opt/AgentPipeline`）。
- 手工面：一次「修复轮 + 紧接着一次部署」的连跑，两端都不需要人 ssh 上去清分支。

**Status:** needs-triage

**Blocked by:** None
