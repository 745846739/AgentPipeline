# 监控记录:任务 01M3X472FJF8NW9082K6BFZPKC

- 标题:基于上一轮 ux-audit:源码+真实页面深挖 UX 改进点(第三轮)
- 服务器:106(root@106.12.12.6,服务 agent-pipeline,端口 3389)
- 查询方式(ssh 内,回环豁免配对闸门):
  `curl -sk https://127.0.0.1:3389/tasks/01M3X472FJF8NW9082K6BFZPKC`(detail / flow / metrics)
- 创建时间:2026-10-02T01:38:45Z,worktree `/root/.agentpipeline/worktrees/01M3X472FJF8NW9082K6BFZPKC`

## 基线快照(2026-10-02,监控开始时)

- 状态 `running`,阶段 `architect-design`,节点 `execute`(run=150,attempt=1)
- 过渡史:init → validate_input → execute,全部 `normal` 触发,无重试
- 指标:init 均值 1.0s;architect-design 均值 27.8s、2 runs、重试率 0
- 日志检查:journalctl 近 30 分钟无 ERROR/WARN/panic,agentpipeline.log ERROR 计数 0
- **观察项 O1**:`/tasks/{id}/metrics` 里 `total_calls=2` 但 `stored_total_calls=1`、
  `total_tokens=46028` 与 `stored_total_tokens` 相同——两个计数字段不同步,待确认是
  统计口径差还是落库滞后(如为 bug,后续修)。
- **观察项 O2**:validate_input 一次通过耗时 ~56s(01:38:54→01:39:50),对校验节点
  偏长,后续看是否为普遍水平(优化候选)。

## 巡检记录

- **02:05(本地 10:05,UTC 02:0x)第一次巡检**:大幅推进。01:54 architect-design 完成
  (含 validate_output),01:55:30 起 develop-design / test-design 并行,01:59:23 sync-check
  汇聚通过后 `auto_resume` 进入 develop。当前 run=160(develop/execute,**attempt=2**,
  02:01:00 起),develop 指标 retry_rate=0.5(2 runs)。validate_first_pass_rate=1.0,
  stalled=false。无 ERROR/panic。
- 新证据:20 分钟内 **5 条 `submit_metadata` 截断救援 WARN**(见汇总 B1);
  develop attempt=2 的重试原因日志里未见明确报错行(见汇总 B2)。
- O1 更新:`total_calls=11` vs `stored_total_calls=10`,token 两处仍相同——差距稳定为
  1 次 call、0 token,更像「进行中那次 call 未落库」的口径差而非 bug(观察降级)。
- 阶段耗时:architect-design 3 runs 均值 331.9s;develop-design 73.6s;test-design 60.0s;
  develop 48.6s(尚只有 2 runs,继续观察)。
- **02:26(本地 10:26)第二次巡检**:无阶段变化,仍在 develop/execute(run=160,
  attempt=2,02:01:00 起,已连续 ~25 分钟,无 stalled)。近 20 分钟该任务在 journalctl
  里**零条日志**——节点事件与 WARN 都没有,应是一次长模型调用进行中,不是卡死
  (updated_at 仍在推进,02:22:53)。develop 指标未变(2 runs,retry_rate 0.5),B2 无
  新证据。
- **O1 收口**:`total_calls=11` 与 `stored_total_calls=11` 已对齐(上次差 1 的那次
  call 落库了),确认是「进行中 call 未落库」的口径差,**不按 bug 跟踪**。
- **02:47(本地 10:47)第三次巡检**:上一轮「疑似卡死」**排除**。`/tasks/{id}` 的
  `updated_at`/token 虽停在 02:22:53,但 `agentpipeline.log` 里 run=160 的模型请求
  一直在正常派发/收场(request=2795→2800,最后一发 02:41:20 status="ok")。任务在
  develop 阶段持续干活,进程健康;worktree 里还起了它自己的 debug 服务
  (`target/debug/agent-pipeline serve --port 0`,对应任务描述里的「起本地服务走查
  真实页面」,属预期)。
- 新观察:**B3(可观测性缺陷)任务详情在长节点期间看起来是冻住的**——
  `updated_at`/`total_tokens` 不随模型请求更新(这次静止 45 分钟差点误判 stalled),
  `stalled` 字段也没帮上忙;而「模型请求派发/收场」只写文件日志,journalctl 里没有。
  优化方向:节点内每次请求收场时轻量更新 updated_at,或在详情里透出「最近一次
  模型请求时间」。
- 新观察:**B4(性能,优化候选)kanban_foreman_messages 慢查询**:02:26:29 sqlx
  WARN,子查询 `WHERE session_id=? ORDER BY id DESC LIMIT ?` 外层再倒序,1.43s 超过
  1s 阈值。疑似 session_id 缺索引或外层把 content/thinking 等大列也带进来排序。
- 新观察:**O3(watch)磁盘水位骤降**:02:22:53 磁盘剩 9.23GB → 02:26:29 剩
  8.49GB,3.5 分钟掉了 ~740MB。历史上有过磁盘 71% 的教训,且 develop 代理正在
  worktree 里构建/跑服务,持续这个速率几小时就会见底。下轮巡检对比 disk_free。
- **03:07(本地 11:07)第四次巡检**:develop/execute 仍在干活(run=160,模型请求
  已到 request=2829,03:03 收场 status="ok",节奏正常;develop 已持续 ~68 分钟,
  与「真实页面走查+截图」的工作量相符)。journalctl 近 20 分钟该任务零 WARN/ERROR。
- **O3 归因(暂稳)**:磁盘 82% 用量、剩 7.0GB(上轮 8.49GB)。大头=任务 worktree
  的 `target/`(debug 构建)1.4GB + node_modules 101MB,与两轮掉量吻合;debug 服务
  已起,构建应已收尾,掉速有望止住。全程盘面:/usr 14G、/root 6G、/opt 5.7G,
  40G 盘用了 31G。
- 新观察:**B5(清理机制缺失,改进候选)任务 worktree 的构建产物不回收**——本任务
  在 worktree 里 cargo build 出 1.4GB 的 `target/`,任务归档后没有清理路径;106 只有
  40G 盘,几个跑过构建的任务叠起来就能吃穿。优化方向:任务 done/archive 时清
  worktree 的 `target/`(或至少 debug 档),或给 worktree 换共享 target/缓存。
- **03:26(本地 11:26)第五次巡检**:develop/execute 继续干活(模型请求已到
  request=2845,03:23 派发中;run=160 attempt=2,develop 已 ~85 分钟)。journalctl
  近 20 分钟零 WARN/ERROR。`/tasks` 的 `updated_at` 仍停在旧值,再次印证 B3。
- **O3 收口(本轮解除)**:磁盘从 7.0GB free 回升到 **12GB free(70%)**。实查:
  worktree 的 `target/` 已不存在(应是被 develop 代理自己清了,或盘压清理机制)。
  但注意这次是「恰好被清」,不是机制保证——**B5 仍成立**:没有归档时清理
  worktree 构建产物的确定路径。
- 观察:worktree `.scratch/` 里尚无 `ux-audit-3` 目录,最终交付物(改进点清单)
  未落盘,与任务仍在 develop 相符。develop 单节点已 85 分钟——**O4(优化候选,
  待收尾确认)**:走查型任务在 develop 节点一跑就是小时级,后面 review/test 阶段
  是否还需要完整过一遍值得看;先不判问题,等终态看总时长分布。
- **03:47(本地 11:47)第六次巡检**:重大事件——**develop run=160 于 03:31:07
  节点超时**(transition id=127,reason「节点超时,自动续接上一轮转录(连续第 1 次
  超时)」,trigger=timeout),按续接机制起新 run=161 attempt=3,之后模型请求正常
  (request=2912→2914 均 ok)。run=160 从 01:59 跑到 03:31 约 92 分钟后超时,与 O4
  呼应:走查型工作在单节点里顶到了节点时限。
- **B3 新证据(计数在节点边界才落库)**:03:31 节点收口瞬间 `total_tokens` 从
  3,495,108 一步跳到 **17,562,058**(+1400 万),而 `total_calls` 仍停在 11——run=160
  期间百余次模型请求的 token 全部压到节点结束时一次入账。中途看任务的人(包括
  本监控)完全无法从详情判断真实消耗;这是 B3「updated_at 不更新」的同一根因,
  修 B3 时应一起修:请求粒度滚动入账。
- **O3 后续**:磁盘继续回升到 **19G free(51%)**。归因:/opt 5.7G→3.0G
  (~2.7G,应为 /opt/AgentPipeline 构建缓存被清),其余来自缓存目录;运行中服务
  不受影响(下次 deploy 会重建)。磁盘警报解除,继续留意即可。
- **04:07(本地 12:07)第七次巡检**:run=161 仍在 develop/execute 干活(request 到
  2949,均 ok),无阶段推进,无 ERROR。04:00:35 一条 WARN:**模型调用传输类失败
  就地重发**(resent=1/budget=2,error=连不上 provider)——决策 373 的传输类重发
  路径实际生效,掐流没花掉整轮,重发后 04:03 起请求恢复正常。说明 106→provider
  链路偶发断连,兜底按设计工作;暂记为「机制验证通过」而非缺陷。
- 新观察:**B6(bug 候选)超时续接后活计数倒退**:`/tasks/{id}/metrics` 里
  `total_tokens=3,495,108`(旧值)而 `stored_total_tokens=17,562,058`——活计数比
  落库值还小 1400 万,应是一次性大额入账后,续接路径把内存计数重置回了旧快照。
  `/tasks/{id}` 详情仍是 17,562,058,两个端点口径打架。与 B3/B6 同族:计数系统的
  「活值 vs 落库值」没有一个一致的真相源,建议一起修。
- 磁盘稳定 19G free(51%)。develop 阶段指标:3 runs,均值 1835s,retry_rate 0.67。
- **04:27(本地 12:27)第八次巡检**:**服务在 04:12:18 UTC 被整体重启**(pid
  706924→843771)。死因待查:journal 里是干净的 Stopped→Started 对,无 panic/OOM/
  信号退出行;dmesg 无 OOM;提交号未变(f2dcf27)、无部署动作——不像崩溃也不像
  deploy,像有人/某机制跑了 `systemctl restart`,**来路不明(B7 待查)**。前 48 秒
  有一条可疑前兆 WARN(04:11:30「resume 触发被在跑的 executor 持续挡下,放弃本次
  触发」)。另注意 106 的 22 端口一直有随机 IP 的 preauth 噪音,未见成功登录。
- 重启后的表现是好消息:应用启动恢复机制把中断的 running 任务自动归队
  (「已将中断的 running 任务归队待调度 count=1」),run=162 attempt=4 接着干,
  模型请求到 2982 均 ok——**长任务扛进程重启不丢,恢复路径实测通过**。
- **B8(语义问题,观察)attempt 计数器混义**:develop 的 attempt 已到 4,但三次
  增长分别是:传输类失败就地重发后的轮内重试(02:01)、超时续接(03:31)、进程
  重启恢复(04:12)——三种完全不同的「重入」共用一个 attempt 数字,看指标无法
  区分哪种;建议 attempt 只表「同轮重试」,续接/恢复走独立字段。
- 交付物仍未落盘(`.scratch/` 里只有 ux-audit / ux-audit-2)。develop 节点累计
  已 ~2.5 小时(三次重入)。
- **04:50(本地 12:50)第九次巡检——发现 P1 级问题**:**服务 HTTP 层自 04:12 重启后
  挂死**。证据:进程 843771 活着、0.0.0.0:3389 在听,但主线程 0.0% CPU 停在
  `futex_wait_queue`,**listen 积压 21 个连接全部得不到 accept**;回环上 `/`、
  `/tasks`、`/tasks/{id}`、metrics 全部 8s 超时(curl 000)。而后台执行体不受影响
  ——run=162 模型请求一路 ok 到 request=3007(04:47)。时间线:04:07 巡检 API 还
  正常,04:12 重启后不久 HTTP 即死,持续 35 分钟以上未被察觉(没有探针在盯这个)。
  定为 **B9(P1)重启后 HTTP 死锁**:执行体与 HTTP 服务面在同一进程里分离存活,
  主线程/accept 路径死锁,极像共享锁(如 SQLite 写锁或全局互斥)被执行体长持或
  重启恢复路径拿了不放。改进方向:① 找根因修死锁;② 加「回环 health 探针 +
  挂死自动重启」兜底(WatchdogSec 或外部探活);③ HTTP 挂死期间任务还在烧 token
  却无人可见,与 B3 的可观测性问题叠加后果更重。
- **B7 归因推进(部分解开)**:00:22:58 UTC 的 deploy run 36945644581(f2dcf27,
  check 通过自动触发)25 秒完成重建+重启——pid 706924 是它起的。04:12:18 的第二
  次重启仍无主:无部署 run、无交互登录(last 只到 9/29)、无 OOM/panic;`last` 看
  不到非交互 ssh 执行,故**持有密钥的自动化或人工手动 `systemctl restart` 都可能**。
  留给用户确认;若非本人操作,需要排查密钥使用痕迹(/root/.ssh/authorized_keys、
  auth.log)。
- **05:01(本地 13:01)处置:经用户同意重启服务**。`systemctl restart agent-pipeline`
  后一切恢复:pid 867866,`/` 与 `/tasks/{id}` 均 200(14ms),任务自动归队,
  run=163 attempt=5 于 05:01:40 在 develop/execute 继续跑,token 计数 17,562,058
  未丢。**B7 关闭**:用户确认重启来自看板任务本身(任务在干活时重启了主服务,
  04:12:18 那次即此)——这也解释了「无登录、无部署 run、干净 Stop→Start」:
  应用以 root 跑,任务执行 `systemctl restart` 不会留下登录痕迹。
- **B9 遗留(后续排查,未修)**:重启后 HTTP 死锁的根因还在。已存证:04:12:18
  重启 → HTTP 数分钟内挂死(主线程 futex_wait、0% CPU、backlog 积压 21),执行体
  继续跑 ~49 分钟(request 到 3007+)直到人工重启;本次人工重启后 HTTP 正常、
  执行体也正常。排查建议:① 复现路径疑为「任务内重启服务 → 新进程恢复执行体
  与 HTTP 面并发初始化 → 某锁(疑 SQLite 写锁/全局互斥)在恢复路径被长持」;
  ② 给 accept/HTTP 面加慢日志或独立线程池;③ 兜底:回环 health 探针 + 挂死自动
  重启(WatchdogSec 或外部探活);④ 值得核对该时段是否与「任务归队恢复 run=162」
  的窗口重合(04:12:28 run=162 起跑,HTTP 挂死发生在其后数分钟内)。
- **05:10(本地 13:10)第十次巡检**:平稳。run=163 在 develop/execute 干活
  (request 到 3027,均 ok),无阶段推进,零 WARN/ERROR,磁盘稳定 19G。交付物
  仍未落盘。
- **05:45(本地 13:45)B9 二次复发 + 处置**:用户报告 Web 打不开,实查确认——
  回环 curl 超时、主线程 futex_wait 0% CPU、backlog 积压 13,执行体照常
  (request=3082)。05:39:47 人工重启恢复(pid 889283,200,任务归队,run 163 按
  「进程退出还在跑」标终态后重续)。
- **B9 关键修正:不是「重启才触发」**。本次距 05:01 重启已 38 分钟、期间无任何
  重启,HTTP 在 05:27(巡检实测健康)到 05:44 之间**静默挂死**:窗口内 journal
  零 WARN、零慢查询、零报错,recording 的模型请求全程未断。共同因素只剩
  「develop 执行体长跑中」——两次挂死分别发生在执行体活跃 3~15 分钟
  (run=162)与 26~43 分钟(run=163)后,无固定时长,像偶发的锁竞争或资源耗尽
  (如某请求把 worker 线程全占)。挂死时进程 CPU 累计仅 4min28s/38min,不是
  忙死是等死。**B9 升级为最高优先级**:Web 完全不可用是用户可感知故障,且每次
  都要人工重启。
- 停血方案(待用户点头):106 上加回环探活 + 自动重启(每分钟 `curl -m 5 -sk
  https://127.0.0.1:3389/` 失败连续 2 次即 `systemctl restart agent-pipeline`,
  恢复机制已三次实测可靠);治本仍要找死锁根因。
- **决定(用户,06:00 前后)**:**不装探活止血**,B9 直接治本。治本时的已存证据
  汇总:两次静默挂死均在 develop 执行体活跃数分钟~四十余分钟后发生(3~15min /
  26~43min),主线程 0% CPU 停在 futex_wait、listen backlog 积压、journal 零
  WARN、recording 模型请求不断;一次紧随「任务运行中重启→归队」(04:12),一次
  无重启纯运行中(05:2x~05:4x)。排查入口建议:HTTP 面与执行体共享的锁
  (SQLite 连接池/全局互斥)、SSE stream 端点(`/tasks/{id}/stream`)的订阅路径、
  以及看门狗/resume 轮询与 accept 的相互作用;复现手段:任务运行中反复重启 +
  长时间 develop 压测。
- **06:06(本地 14:06)第十三次巡检:B9 第三次复发**。06:00 实测还健康(14ms),
  06:06 已 8s 超时——挂死发生在 run=164 起跑(05:39:57)后约 20 分钟,**与执行体
  活跃强相关的第三个样本(3~15min / 26~43min / ~20min)**,无固定时长,进一步
  排除「固定定时器」假说,更像与特定请求序列/工具调用相关的偶发锁竞争。06:04:25
  人工重启恢复(pid 903869,200,任务归队继续)。用户已定:不装探活,直接治本。
- **06:26(本地 14:26)第十四次巡检**:无变化。06:04 重启后 HTTP 健康(16ms),
  run=165(attempt=7)develop 干活中(request 到 3134,均 ok),零 WARN/ERROR,
  磁盘稳定 19G,交付物未落盘。
- **06:47(本地 14:47)第十五次巡检**:无变化。HTTP 健康(17ms,重启后 43 分钟
  未挂死——run=165 已超前面样本的最短挂死时长),run=165 develop 干活中
  (request 到 3177,均 ok),零 WARN/ERROR,磁盘稳定,交付物未落盘。
- **07:07(本地 15:07)第十六次巡检**:无变化。HTTP 健康(557ms,重启后 63 分钟),
  run=165 develop 干活中(request 到 3202,均 ok),零 WARN/ERROR,磁盘稳定,
  交付物未落盘。run=165 已连续跑 63 分钟未触节点超时(上上次 92 分钟超时),
  接近临界,下轮关注。
- **07:27(本地 15:27)第十七次巡检**:无变化。HTTP 健康(22ms,重启后 83 分钟),
  run=165 develop 干活中(request 到 3207,均 ok,已连续 83 分钟未超时),零
  WARN/ERROR,磁盘稳定,交付物未落盘。
- **07:47(本地 15:47)第十八次巡检**:run=165 于 07:34:36 触发**节点超时自动续接
  (连续第 2 次超时,跑了 90 分钟)**,新 run=166(attempt=8)接续干活(request 到
  3253,均 ok)。HTTP 健康(14ms),零 WARN/ERROR,磁盘稳定,交付物未落盘。
  90 分钟的节点时限与 run=160 的 92 分钟吻合,确认 develop 节点时限 ~90min;
  走查型工作两轮都顶满时限,支撑 O4(节点粒度对这类任务偏粗)。
- **08:07(本地 16:07)第十九次巡检**:无变化。HTTP 健康(14ms,重启后 123 分钟),
  run=166 develop 干活中(request 到 3288,均 ok),零 WARN/ERROR,磁盘稳定,
  交付物未落盘。
- **08:27(本地 16:27)第二十次巡检**:无变化。HTTP 健康(14ms,重启后 143 分钟),
  run=166 develop 干活中(request 到 3315,均 ok;已跑 53 分钟),零 WARN/ERROR,
  磁盘稳定,交付物未落盘。
- **08:47(本地 16:47)第二十一次巡检**:无变化。HTTP 健康(14ms,重启后 163 分钟),
  run=166 develop 干活中(request 到 3323,均 ok;已跑 73 分钟,接近 90 分钟时限),
  零 WARN/ERROR,磁盘稳定,交付物未落盘。
- **09:07(本地 17:07)第二十二次巡检**:无变化。HTTP 健康(14ms,重启后 183 分钟),
  run=166 develop 干活中(request 到 3327,均 ok;已跑 93 分钟,超过既往 ~90 分钟
  时限仍未续接,时限或为软限/按情况浮动),零 WARN/ERROR,磁盘稳定,交付物未落盘。
- **09:20(本地 17:20)专项检查:读会话与命令流水,判断是否跑偏**。结论——
  **方向对,但陷在「超时→续接/重跑→重新侦察」循环,交付物迟迟不落盘**:
  1. 干的活是对的:对照前两轮票据(README 数字、PNG 计数)、grep 源码(TaskDetail
     tab、board filter、resetTaskContent、SettingsNotify VAPID)、用 UX_AUDIT=1/
     UX_AUDIT2=1 跑 playwright 走查、重新取证截图——与任务描述一致。
  2. 改了 `frontend/e2e/ux-audit.spec.ts`(+22/−8):理由成立(推荐面板路由随
     决策 198 迁到 #/settings/stages,旧选择器已失效),注释里写明了改动原因并
     保持截图文件名不变——属修审计工具而非改产品代码,但验收要求「不直接改代码」,
     **交付时必须显式列出这处 diff** 让人复核。
  3. 低效循环的证据:develop 三轮大 run(160/165/166)prompt_tokens 分别 1387万/
     1215万/1491万——每轮都整卷转录重喂;160、166 相继超时,09:04:38 出现
     「续接两轮未恢复,**空白重跑一次**」;每轮重置后都重新做侦察(git status/log、
     freshness 检查、ux-audit-3 MISSING 反复确认、重跑 playwright),90 分钟耗尽
     又没到写清单那一步。7.5 小时过去 `.scratch/ux-audit-3/` 仍不存在。
  4. 每轮 ~1400 万 prompt token 的重喂成本也该进 O4 的账:这个任务形态(长走查)
     在「整卷转录 + 90 分钟节点」的机制下,每轮都在给已做过的工作付费。
- **09:27(本地 17:27)第二十三次巡检**:无实质变化。空白重跑轮 run=167
  (attempt=9,09:04:38 起)在 develop 干活(request 到 3370,均 ok),HTTP 健康
  (14ms),零 WARN/ERROR,磁盘稳定;`ux-audit-3` 仍 MISSING。
- **09:35(本地 17:35)终态:用户决定停止任务**。POST /cancel → `ok:true`,
  status=**cancelled**(run=167 中止)。任务共运行 01:38–09:04+ 约 7.6 小时,
  develop 阶段 9 次重入、4 次节点超时(160/165/166/167,~90min/次)、2 次续接
  失败后 1 次空白重跑,交付物 `ux-audit-3` 最终未落盘;worktree 保留
  (含修改过的 frontend/e2e/ux-audit.spec.ts +22/−8,未提交)。
  监控转入「源码优化分析」阶段,巡检暂停。

## 源码优化方案(2026-10-02,探查 agent 定位到 file:line)

- **P0|B9 HTTP 挂死(根因基本锁定)**:`crates/app/src/serve.rs:252-269` 手写
  TLS accept 循环是**串行**的——`TcpListener::accept` 后在同一任务里 `await`
  TLS 握手,且**无超时**。公网扫描器建 TCP 后不发 ClientHello(半开连接),
  一次握手永久挂住 → accept 停摆 → backlog 积压 → 全站超时;执行体在别的
  task 里完全不受影响。时间线吻合:重启后重新暴露公网即被探测(3~15 分钟),
  纯运行中也随时可能被扫到(26~43 分钟)。**修法**:每连接 spawn 握手 +
  `tokio::time::timeout`(如 10s)包住握手,握手失败即断;或改并发 accept。
- **P1|B3/B6 计数**:`refresh_task_totals`(`core/src/storage/tasks.rs:407-419`)
  只在边界调用(`model_invoke.rs:286/312/337` 等收口点)——把刷新挂到已有的
  心跳 `touch_run_heartbeat`(`model_invoke.rs:884`)即可滚动入账。B6 倒退根因:
  `metrics.rs:33-40` 的去双算规则按「被续接 run 指认」整条排除历史 run,
  把超时 run **已实烧的 token 也从账面抹掉**——排除条件应改为「其 token 已被
  后继计入」而非「被指认」。
- **P1|O4 全量重喂**:run 的 prompt_tokens 是轮累计(`model_invoke.rs:877`
  每轮 add),而每轮 `plan.request(&trace.messages)`(`model_request.rs:388-404`)
  整卷转录原样发,O(n²)。L3 压缩有三个松口(容量依赖 provider 登记的
  context_window、`keep_recent_rounds=5` 把大工具输出全保住、撞窗自校准只上调
  `model_invoke.rs:837-858`);续接素材整卷反序列化
  (`run_ledger.rs:249-279`)。方向:压缩触发改按转录体量兜底、续接/空白重跑
  改带紧凑简报而非全卷、走查型任务在节点内强制阶段性落盘。
- **P2|B1 submit_metadata 截断**:截断发生在**上游网关**(`finish_reason=length`
  把 tool_calls 腰斩,`metadata.rs:18-29`/`client.rs:185-192` 自证,106 落库
  样本里正文 `<tool_call>` 是全的)——本地已有三级降级;改进点:① 救援后把
  缺失字段清单写进任务事件而非静默;② `providers/mod.rs:480` 的
  `from_utf8_lossy` 按网络分块转换会切坏多字节字符,应缓存不完整字节尾。
- **P2|运行时卫生**:HTTP handler 里的裸同步文件读
  (`routes/tasks.rs:1018/:1050`)应走 spawn_blocking;巨型转录的同步 JSON
  克隆/序列化/字符级 token 估算(`model_request.rs:395`、`agent/context.rs:125-147`)
  在 tokio worker 上秒级不可分片,B9 修掉后仍是饿死隐患;
  `git.rs:105-115` blocking 池超时后线程不回收。
- 详细定位过程与全部 file:line 见本次探查结论(已在本节浓缩)。
- **06:00(本地 14:00)第十二次巡检**:无变化。05:39 重启后 HTTP 保持健康
  (详情接口 14ms),run=164(attempt=6)在 develop 干活(request 到 3094,均 ok),
  零 WARN/ERROR,磁盘稳定 19G,交付物未落盘。develop 已六次重入,任务整体已跑
  4.4 小时仍未收口——O4 权重上升。
- **05:27(本地 13:27)第十一次巡检**:无变化。run=163 develop/execute 干活中
  (request 到 3058,均 ok),零 WARN/ERROR,磁盘稳定 19G,交付物未落盘。

## 汇总:bug 与优化点

- **B1(bug 候选,高频)`submit_metadata` 参数 JSON 截断**:
  2026-10-02 01:54:43 / 01:55:14 / 09:58:04(UTC 01:58:04)/ 01:58:26 / 01:59:10 共
  **5 条 WARN**(`agentpipeline_core::agent::tools`),横跨 architect-design、test-design、
  develop-design 三阶段,raw_len 分别 1617 / 29 / 7748 / 33 / 29 字节,全是
  `EOF while parsing`。两个方向都值得查:
  1. raw_len=29/33 两次出现——**很短的参数也在截断**,不太像单纯的输出长度截断,
     可能是模型侧工具调用参数拼装/流式切片的 bug;
  2. 救援逻辑「被截字段不会自己回来」意味着**字段静默丢失**,任务拿到的 metadata
     可能缺内容但流程照走,无后续校验兜底。优化方向:截断救援后应要求模型补交
     或至少把缺失字段清单写进任务事件;短参数截断查传输/拼装层。
- **B2(待查)develop 首跑 attempt=1 无说明就重试**:02:01:00 起 run=160
  attempt=2,而 01:59:23 的 run=159 attempt=1 之后日志里没有对应的报错/原因行,
  指标 develop retry_rate=0.5。需确认这是模型轮失败就地重发(决策 373 那条
  传输类重发路径)还是其他原因——若每次失败都不留原因行,排障会缺证据,
  建议重试时补一条 WARN 级日志。下次巡检看 develop 是否继续重试、并回查
  observability 里 run=159 的结束记录。
