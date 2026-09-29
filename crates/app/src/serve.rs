//! 服务启动（决策 153⑤：起服逻辑沉入 lib，供二进制、冒烟测试与桌面壳复用）。
//!
//! 二进制入口只做参数解析后调用 [`serve`]；绑定支持端口 `0`——此时内核分配真实端口，
//! 由 [`ServerHandle::port`] 与启动日志给出，调用方不再需要「先探测空闲端口再释放」
//! 的绕开（决策 153⑤；决策 128 的同源 origin 判定也依赖这个真实端口）。

use std::sync::Arc;
use std::time::Duration;

use agentpipeline_core::agent::repo::Libgit2Repo;
use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::{normalize_origin, Config, LogFormat};
use agentpipeline_core::home::{
    check_permissions, restrict_file_permissions, restrict_permissions, Home,
};
use agentpipeline_core::pipeline::ForemanRunner;
use agentpipeline_core::sse::SseBus;
use agentpipeline_core::storage::Store;
use anyhow::Context;

use crate::build_router;
use crate::runtime::Runtime;
use crate::state::{AppState, RebindRequest};

/// 已就绪的服务句柄：真实绑定地址可读，`shutdown` 置位触发优雅退出。
pub struct ServerHandle {
    /// 内核分配的实际端口（绑定 `:0` 时为真实值，不再是 0）。
    pub port: u16,
    /// 请求停机（决策 54：第一次 SIGINT 走这里）。
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// 服务任务；正常停机后 resolve。
    pub server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// 绑定监听地址并回读真实端口（决策 153⑤）。
///
/// 端口 `0` 时由内核分配真实端口，返回值即真值；调用方（[`AppState`] 的同源 origin
/// 白名单、启动日志、子进程就绪行）都必须用它，不得沿用配置里的 `0`。
pub async fn bind_listener(
    host: &str,
    port: u16,
) -> anyhow::Result<(tokio::net::TcpListener, std::net::SocketAddr)> {
    let addr = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("无法解析绑定地址 {host}:{port}"))?
        .next()
        .context("绑定地址解析为空")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("端口被占用或无法绑定：{addr}"))?;
    let bound = listener
        .local_addr()
        .context("无法读取内核分配的实际端口")?;
    Ok((listener, bound))
}

/// 首选端口 + 「占用则退让」的绑定（决策 213），返回实际监听器、真实端口，以及**这个端口是谁给的**。
///
/// 为什么退让要单独成为一个选项，而不是让所有调用方都退让：`--port` 是用户明确指定的，
/// 绑不上就该按 [`bind_listener`] 的报文报错（「端口被占用或无法绑定」是唯一不需要猜的错误），
/// 悄悄换一个端口等于把这条信息吃掉。桌面壳那条路不同——它没有命令行，界面上的端口也不是
/// 用户当下选的，**应用打不开比端口变一次更糟**，故它显式打开这个开关。
///
/// 退让**只在 `EADDRINUSE` 时发生**：权限不足、地址不存在之类的错误照旧报出来——那不是
/// 「换个端口就能好」的事，退让只会把真正的原因藏起来。
///
/// 退让的代价是**分享地址会变**（手机上的旧书签下次启动就打不开），故这里 warn 出声，
/// 并经 [`PortSource::Fallback`] 一路报到 `/server-info`，由分享页对使用者说清楚。
async fn bind_preferred(
    host: &str,
    port: u16,
    source: crate::state::PortSource,
    fallback_to_ephemeral: bool,
) -> anyhow::Result<(
    tokio::net::TcpListener,
    std::net::SocketAddr,
    crate::state::PortSource,
)> {
    use crate::state::PortSource;
    match bind_listener(host, port).await {
        Ok((listener, bound)) => Ok((listener, bound, source)),
        Err(e) if fallback_to_ephemeral && is_addr_in_use(&e) => {
            tracing::warn!(
                port,
                error = %e,
                "首选端口被占用，退让到内核随机端口——分享地址会变，手机上的旧网址需重新扫一次"
            );
            let (listener, bound) = bind_listener(host, 0).await?;
            Ok((listener, bound, PortSource::Fallback))
        }
        Err(e) => Err(e),
    }
}

/// 错误链里有没有 `EADDRINUSE`（端口被占用）。
///
/// 看错误链而不是报文：`bind_listener` 给它套了一层「端口被占用或无法绑定」的上下文，
/// 但那条上下文同时也罩着权限、地址不可用等**不该退让**的失败，按报文判断必然误判。
fn is_addr_in_use(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    })
}

/// 把 router 变成能向处理器提供对端地址的 service（决策 182）。
///
/// 返回类型本身就是行为事实：裸 `Router` 交给 `axum::serve`（`into_make_service`）时
/// 对端地址不会进入请求扩展，[`crate::peer::peer_address`] 便没有东西可读——只有
/// `into_make_service_with_connect_info` 会让 `ConnectInfo<SocketAddr>` 出现（票 07 的
/// 「仅回环可读」依赖它）。抽成具名函数是为了让这处接线本身可断言、可被下游引用。
pub fn into_serving_service(
    router: axum::Router,
) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::Router, std::net::SocketAddr>
{
    router.into_make_service_with_connect_info::<std::net::SocketAddr>()
}

/// 改绑时给在飞请求的优雅窗口（决策 186）。
///
/// 超过它就 `abort` 掉监听任务。**这个超时不是保守起见，而是必需的**：SSE 流
/// （`/tasks/{id}/stream`、`/foreman/stream`）在客户端断开前永不结束，而浏览器的流是
/// 常态——只看「优雅停机」的话，改绑会一直等下去，界面上的按钮永远转圈。前端有带退避的
/// 重连循环（`realtime/connection.ts`），所以切断流的代价是一次自动重连，不是数据丢失。
const REBIND_GRACEFUL_WINDOW: Duration = Duration::from_millis(500);

/// 监听地址的解析：**启动期覆盖 > 界面设置 > 配置文件**（决策 186）。
///
/// 抽成纯函数是为了让优先级本身可断言——三种来源各有一条腿，而「谁盖谁」写错的表现是
/// 「改了没生效」，那是最难从现象反推的一类错误。
pub fn resolve_bind_host(
    startup_override: Option<String>,
    settings_override: Option<String>,
    config_host: &str,
) -> (String, crate::state::BindSource) {
    use crate::state::BindSource;
    if let Some(host) = startup_override {
        return (host, BindSource::Startup);
    }
    if let Some(host) = settings_override {
        return (host, BindSource::Settings);
    }
    (config_host.to_string(), BindSource::Config)
}

/// 一个已起好的监听器（决策 186：可被主管停掉并换一个新的）。
struct Listener {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

/// 绑定并开始接受连接。返回错误时**没有**任何副作用（调用方据此回滚）。
async fn spawn_listener(
    router: axum::Router,
    host: &str,
    port: u16,
) -> anyhow::Result<(Listener, u16)> {
    let (listener, bound) = bind_listener(host, port).await?;
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        axum::serve(listener, into_serving_service(router))
            .with_graceful_shutdown(async move {
                let _ = stop_rx.changed().await;
            })
            .await
            .context("axum 服务异常退出")
            .map_err(|e| tracing::error!(error = %e, "监听任务退出"))
            .ok();
    });
    Ok((
        Listener {
            stop: stop_tx,
            task,
        },
        bound.port(),
    ))
}

/// 停掉一个监听器：先给优雅窗口，超时则强制中断（理由见 [`REBIND_GRACEFUL_WINDOW`]）。
async fn stop_listener(listener: Listener) {
    let _ = listener.stop.send(true);
    let mut task = listener.task;
    if tokio::time::timeout(REBIND_GRACEFUL_WINDOW, &mut task)
        .await
        .is_err()
    {
        tracing::info!("优雅窗口内仍有连接（多半是 SSE 流）：强制断开并改绑");
        task.abort();
    }
}

/// 监听器主管的全部私有状态（决策 186）。
struct ListenerSupervisor {
    state: crate::state::AppState,
    /// 当前生效的监听器；`None` = 换绑过程中正处在「旧已停、新未起」的那一瞬。
    current: Option<Listener>,
    /// 当前端口。**改绑不改端口**：分享页的二维码、手机的书签、桌面窗口的地址都指着它，
    /// 换一个端口等于让刚扫过的码失效。
    port: u16,
    /// `[server] host` 的声明值——「恢复配置文件的值」回到的就是它。
    config_host: String,
    /// 启动期覆盖（`--host` / `AGENTPIPELINE_LAN`）。有它时界面设置改得动**这一次**，
    /// 但重启后仍由它说了算，故界面要能说出这件事（`/server-info` 的 `bind_source`）。
    startup_override: Option<String>,
}

impl ListenerSupervisor {
    /// 换绑：停旧 → 绑新 → 起新；**绑新失败则回滚到旧地址**（决策 186）。
    ///
    /// 顺序是「先停后绑」而不是「先绑后停」：回环与全网卡绑在**同一个端口**上，
    /// 多数平台上两个 socket 不能并存（`0.0.0.0:P` 与 `127.0.0.1:P` 互斥），
    /// 先绑必然 EADDRINUSE。代价是那几毫秒里没有监听者——期间到达的连接被拒，
    /// 浏览器的 SSE 重连循环会补上（决策 186 的残留风险，如实记在文档里）。
    ///
    /// 回滚不是可选项：一次失败的开关不该让整个服务消失。回滚也失败时（端口被别的进程
    /// 抢走）只能报出两段原因——那是「这台机器上端口真的没了」，不是本函数能修的。
    async fn swap(
        &mut self,
        host: &str,
        source: crate::state::BindSource,
    ) -> std::result::Result<u16, String> {
        let previous_host = self.state.bind_host();
        let previous_source = self.state.bind_source();
        if let Some(listener) = self.current.take() {
            stop_listener(listener).await;
        }

        let router = crate::build_router(self.state.clone());
        match spawn_listener(router, host, self.port).await {
            Ok((listener, port)) => {
                self.current = Some(listener);
                self.state.set_bind_host(host, source);
                tracing::info!(host, port, "已改绑监听地址");
                Ok(port)
            }
            Err(bind_error) => {
                tracing::error!(error = %bind_error, host, "改绑失败，回滚到原地址");
                let router = crate::build_router(self.state.clone());
                match spawn_listener(router, &previous_host, self.port).await {
                    Ok((listener, _)) => {
                        self.current = Some(listener);
                        self.state.set_bind_host(&previous_host, previous_source);
                        Err(format!(
                            "无法绑定 {host}:{}（{bind_error}）；已恢复为 {previous_host}",
                            self.port
                        ))
                    }
                    Err(rollback_error) => Err(format!(
                        "无法绑定 {host}（{bind_error}），且回滚到 {previous_host} 也失败\
                         （{rollback_error}）——请重启服务"
                    )),
                }
            }
        }
    }
}

/// 主管循环：串行处理改绑请求，直到全局停机。
async fn run_listener_supervisor(
    mut sup: ListenerSupervisor,
    mut rx: tokio::sync::mpsc::Receiver<RebindRequest>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                // 全局停机：停当前监听器即可（graceful → abort 的窗口与改绑同一条路）。
                if let Some(listener) = sup.current.take() {
                    stop_listener(listener).await;
                }
                return Ok(());
            }
            request = rx.recv() => {
                // 通道两端都在本进程内（端点侧持发送端），关闭只可能发生在停机路径上。
                let Some(request) = request else { return Ok(()) };
                let already_there = request.host.as_deref() == Some(sup.state.bind_host().as_str());
                let (host, source) = match request.host.clone() {
                    Some(host) => (host, crate::state::BindSource::Settings),
                    // 清除界面设置 → 回到「启动期覆盖 > 配置文件」那一级的解析结果
                    None => resolve_bind_host(sup.startup_override.clone(), None, &sup.config_host),
                };
                // 目标与当前一致时不换绑：换一次就切一次连接（浏览器要重连、二维码要重扫），
                // 为了一个没有变化的结果付这个代价没有道理。落库照做（用户可能只想让它记住）。
                let result = if already_there {
                    Ok(sup.port)
                } else {
                    sup.swap(&host, source).await
                };
                // 持久化只在**成功路径**上做：先落库再改绑的话，一次失败的开关会在下一次
                // 启动时生效——「点了没生效，重启却生效了」是这里最坏的一种状态。
                if result.is_ok() {
                    let persisted = match request.host.as_deref() {
                        Some(host) => sup.state.store.set_server_bind_override(host).await,
                        None => sup.state.store.clear_server_bind_override().await.map(|_| ()),
                    };
                    if let Err(e) = persisted {
                        // 本次已经生效，只是重启后不记得——出声，但不改本次的结果（决策 186）
                        tracing::error!(error = %e, "绑定选择落库失败：本次已生效，重启后会回到上一级");
                    }
                }
                // 应答走在**换绑之后**：触发这次改绑的请求本身就在刚被切断的那条连接上，
                // 所以这一应答常常到不了客户端——前端据此把「传输失败」与「真的失败」
                // 分开：失败后重读 `/server-info`，读到目标状态就算成功（决策 186）。
                let _ = request.reply.send(result);
            }
        }
    }
}

/// serve 的启动参数（决策 157）：CLI 覆盖项与扩权 origin；`None` / 空表回落配置文件。
#[derive(Debug, Default, Clone)]
pub struct ServeOptions {
    /// CLI `--port`；`0` = 内核随机分配。
    pub port_override: Option<u16>,
    /// 首选端口被别的进程占着时，退让到内核随机端口（决策 213）。
    ///
    /// 缺省 `false`（命令行与测试的 fail fast 姿态）：显式指定的端口绑不上就该报错。
    /// **只有桌面壳打开它**——桌面应用没有命令行，开不了窗比端口变一次更糟。
    /// 退让的代价是分享地址会随重启变，故它同时被上报成 [`crate::state::PortSource::Fallback`]。
    pub port_fallback_to_ephemeral: bool,
    /// CLI `--host`（局域网访问用 `0.0.0.0`）；缺省回落 `[server] host`。
    pub host_override: Option<String>,
    /// CLI `--allowed-origin` 注入的额外放行 origin（已归一）；
    /// 与 `[server] allowed_origins` 取并集，缺省本机集合恒在（决策 128）。
    pub extra_allowed_origins: Vec<String>,
}

/// 以给定配置启动服务，返回可读回真实端口的句柄。
///
/// [`ServeOptions::port_override`] 为 `None` 时回落 `[server] port`（§10.6.5）；
/// `0` 表示由内核分配，真实端口经 [`ServerHandle::port`] 与启动日志给出（决策 153⑤）。
///
/// 端口**默认可预期**：不给 `port_override` 就用配置里的那个，重启不变（决策 213）。
/// 只有 [`ServeOptions::port_fallback_to_ephemeral`] 打开（桌面壳）时，首选端口被占用才
/// 退让到内核随机端口，并把这件事经 [`crate::state::PortSource::Fallback`] 报到 `/server-info`。
pub async fn serve(options: ServeOptions) -> anyhow::Result<ServerHandle> {
    let home = Home::from_env();
    home.ensure_dirs()?;
    let config = Config::load(&home.config_path())?;
    let settings = config.settings();

    // 权限校验：过宽只告警，不阻断启动（§12.14）
    let wide = check_permissions(&home);
    if !wide.is_empty() {
        eprintln!(
            "警告：以下路径权限过宽（建议 0700/0600）：{:?}",
            wide.iter()
                .map(|(p, m)| format!("{} {:o}", p.display(), m))
                .collect::<Vec<_>>()
        );
    }

    // 日志初始化（§10.6.5 / 票 16）：level + format + file 三者生效；
    // 文件目录在此创建并收紧权限（0700 / 0600，§12.14）。失败不阻断启动。
    init_tracing(&config, &home);

    // `[prompts] dir` 覆盖接入（票 16）：executor 经 `Home::prompts_dir` 取模板目录，
    // 覆盖目录尚未存在时也照常回落内嵌 persona（决策 7）。
    let home = match config.prompts.resolved_dir(home.root()) {
        Some(dir) => {
            prepare_prompts_dir(&dir);
            home.with_prompts_dir(Some(dir))
        }
        None => home,
    };

    // `[skills] dir` 覆盖接入（决策 172）：技能根整体替换，executor 与启动校验
    // 都经 `Home::skills_dir` 取它。若指到已有技能生态目录（如 `~/.zcode/skills`），
    // 不应新建也不应改其权限——只有默认技能根才随家目录骨架建立（见 `ensure_dirs`）。
    let home = match config.skills.resolved_dir(home.root()) {
        Some(dir) => home.with_skills_dir(Some(dir)),
        None => home,
    };

    let store = Store::open(home.clone(), Arc::new(SystemClock)).await?;
    // 离线通知（决策 268 / 270 / 272 / 284）：两级解析（界面单元 > config.toml，
    // 272⑥；礼貌两件自 284② 起同为两级），总开关关死一切（272⑧）。挂接面有两条：
    // attention 的记账漏斗（268②）与值班长回话收口（272②，在 `foreman.rs`）；投递
    // best-effort 在出口内部。解析出错只在日志里报并把这一段关死——通知挂了不能把
    // 机器挂了（268② 的姿态）；设置页会把同一句报文展示出来（报错不静默，272⑧）。
    match store.notify_settings_state().await {
        Ok(notify_state) => {
            match agentpipeline_core::notify::resolve_notify_target(&config.notify, &notify_state) {
                Ok(Some(target)) => {
                    // 礼貌取**解析后的**那一份（284②）——启动这一路也不能漏掉界面单元。
                    let politeness = agentpipeline_core::notify::resolve_politeness(
                        &config.notify,
                        &notify_state,
                    );
                    store.set_notifier(Arc::new(agentpipeline_core::notify::WebhookNotifier::new(
                        target,
                        politeness,
                        Arc::new(SystemClock),
                    )));
                }
                Ok(None) => {}
                Err(e) => tracing::error!("离线通知配置有误，这一段关死：{e}"),
            }
        }
        Err(e) => tracing::error!("读不到离线通知的界面设置，这一段关死：{e}"),
    }

    // 恢复序列（决策 127 / 212）：三步走**唯一那一份实现**（决策 255）——`service` 提议的
    // 「重启」动作按的是同一个函数，两处从此不会给出不同的答案。这里只负责把读数记成日志。
    let readings =
        agentpipeline_core::pipeline::foreman_actions::run_recovery_sequence(&store).await?;
    if readings.cleared > 0 {
        tracing::info!(cleared = readings.cleared, "已清理残留的 executor 持有者");
    }
    // 第二步（决策 127 补全，主流程票 08）：孤儿 running 任务归队，
    // 否则调度器（准入只认 queued）不会接管，任务在重启后永久挂起。
    if !readings.requeued.is_empty() {
        tracing::info!(
            count = readings.requeued.len(),
            tasks = ?readings.requeued,
            "已将中断的 running 任务归队待调度"
        );
    }
    // 第三步（决策 212 / 票 13）：项目级 run 既不在 `requeue_running_tasks` 的归队范围内
    //（那条路按 task_id），也不在 `check_timeouts` 的扫描范围内——两条路都不管的后果是它们
    // 跨重启永生。启动时立刻收一次：心跳停了的直接标终态（带原因）。
    if !readings.abandoned.is_empty() {
        tracing::info!(
            count = readings.abandoned.len(),
            runs = ?readings.abandoned,
            "已把中断的项目级 run 标成终态"
        );
    }

    // 恢复流程第四步（决策 231）：把上一进程遗留的「在飞」模型请求收成终态。
    //
    // **这一步是启动特有的，故留在调用点、不在上面那个共用函数里**（决策 255④）：它的判据
    // `finished_at IS NULL` 没有进程限定，语义就是「上一个实例留下的」；而**运行中**被丢弃
    // 的请求另有承担者——`agent::recording::Settle` 的 `Drop` 会把它收成 `Timeout`。
    //
    // 为什么必须有：`finished_at IS NULL` 是那张表唯一的「还在跑」读数，而进程被强杀时
    // 收场那一次写入永远不会发生——不收口的话，一个**死掉的**请求会永远以「在飞」的样子
    // 出现在诊断包里。那是决策 226③ 要根除的同一类失真，只是方向相反（不是假装 0，
    // 是假装还活着）。标终态而不是删行：它在时间线上真的发生过。
    let orphaned = store
        .orphan_inflight_model_requests("上一个进程退出（或被强杀）时这次请求还没有收场")
        .await?;
    if orphaned > 0 {
        tracing::info!(count = orphaned, "已把上一进程遗留的在飞模型请求标成终态");
    }

    // 恢复流程第五步（票 talk-replay 03）：把上一进程遗留的**在途半截行**标成已中断。
    //
    // 与上一步同一条理由、同一个姿势——**显式修订决策 223**（「不做进程退出那一轮的
    // 落账」）：进程被杀时半截行的收口写永远不会发生，不标的话它会永远以「正在说」的
    // 样子挂在时间线上——那是比「什么都不留」更坏的失真（假装还活着），而 223 立的
    // 那一半（失败账照旧由 `system` 行承载）一个字没改。**只写状态与中断时刻，内容
    // 一个字不动**：thinking 与工具步骤是排查断点的证据；中断是终态，不提供续跑。
    let orphaned_turns = store.mark_orphan_foreman_inflights().await?;
    if orphaned_turns > 0 {
        tracing::info!(
            count = orphaned_turns,
            "已把上一进程遗留的在途半截行标成已中断（票 03，修订决策 223）"
        );
    }

    // 出厂技能与点名的幂等播种（决策 261）：只补缺失、不覆盖用户改过的。
    //
    // 必须在下面的 `validate_startup` **之前**：用户若在阶段配置里声明了出厂技能，
    // 校验要看到技能根里文件已经就位。技能文件写不进去不阻断启动（不声明时 `Skill`
    // 工具有友好回落，同决策 261④ 的代价账）；点名的库写失败随 `?` 上传。
    let seeded = agentpipeline_core::agent::factory::seed_factory_defaults(&home, &store).await?;
    for warning in &seeded.warnings {
        tracing::warn!("出厂技能播种未完成：{warning}");
    }
    if !seeded.skills_written.is_empty() {
        tracing::info!(skills = ?seeded.skills_written, "已种入出厂技能");
    }
    if seeded.pointer_written {
        tracing::info!("已为值班长播种 operate-pipeline 点名（foreman persona_append）");
    }

    // 配置 fail fast（决策 47 / 103 / 134）
    let report = store.validate_startup(&settings).await?;
    if !report.demoted_providers.is_empty() {
        tracing::warn!(?report.demoted_providers, "不受支持的 vendor 已降级 enabled=0");
    }

    // CLI --port / --host / 界面设置 > [server] port / host（§10.6.5 的配置此前被硬编码架空，
    // 决策 128 修订同批对齐；决策 157 补 allowed_origins 并集；决策 186 插入界面设置这一级）
    let server = config.server.clone();
    let port = options.port_override.unwrap_or(server.port);
    // 端口是谁给的（决策 213）：显式 `--port` 是 startup，否则是 `[server] port`。
    // 第三档 `fallback` 只有真的退让过才出现，在 bind_preferred 里定。
    let port_source = if options.port_override.is_some() {
        crate::state::PortSource::Startup
    } else {
        crate::state::PortSource::Config
    };
    let (host, bind_source) = resolve_bind_host(
        options.host_override.clone(),
        store.server_bind_override().await?,
        &server.host,
    );
    let mut extra_origins =
        Vec::with_capacity(server.allowed_origins.len() + options.extra_allowed_origins.len());
    for raw in server
        .allowed_origins
        .iter()
        .chain(options.extra_allowed_origins.iter())
    {
        extra_origins
            .push(normalize_origin(raw).map_err(|e| anyhow::anyhow!("allowed_origin 无效：{e}"))?);
    }

    // 停机信号（决策 54）：一处广播，三处消费——监听器主管、tick 循环、维护循环。
    // 不再有第 4 个接收者直接挂在 axum 上：监听器的停机由主管转达（决策 186），
    // 否则改绑与停机两条路径会各停一次、且停机要等一个已经换掉的句柄。
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    // 生产运行时（票 17）：真实 LLM + executor + scheduler（决策 55）。
    let sse = Arc::new(SseBus::default());
    let runtime = Runtime::new(store.clone(), settings.clone(), sse.clone());
    runtime.spawn_tick_loop(store.clone(), settings.clone(), shutdown_tx.subscribe());
    runtime.spawn_maintenance_loop(store.clone(), settings.clone(), shutdown_tx.subscribe());
    // 完全磁盘访问：**每次启动探一次**（决策 306，票 05）。后台跑、不阻塞启动——
    // 缺授权时应用仍要开得起来、人仍要能被引导；有授权时一个字都不输出。
    Runtime::spawn_disk_access_check();

    // 先绑定再建 state：端口 0 时把内核分配的真实端口交给 AppState，
    // 决策 128 的本机 origin 白名单必须用真实端口（用 0 会拒掉桌面壳的同源请求）。
    //
    // 决策 186 起监听器由**主管任务**持有：界面上的「绑定全网卡」开关要能在运行时换掉
    // 它，而换监听器是 `serve` 的私有知识（端口、graceful→abort 的窗口、失败回滚），
    // 不该泄漏给端点。端点只往通道里投一个请求。
    let (listener, bound, port_source) =
        bind_preferred(&host, port, port_source, options.port_fallback_to_ephemeral).await?;
    let actual_port = bound.port();
    let (rebind_tx, rebind_rx) = tokio::sync::mpsc::channel::<RebindRequest>(4);

    // 技能来源（决策 194）：读的是一个 GitHub 仓，URL 由 `owner/repo` 拼，**没有"未配置"形态**
    // ——故总是注入一个实现。空仓名单是合法配置（= 不从任何仓安装），放行判定读的是仓名单，
    // 与这个实现无关（端点会给出「怎么开」的报文，而不是 500）。
    //
    // 缓存根放在家目录下而不是系统临时目录：取下来的裸仓要跨列表与安装两次请求复用，
    // 落在 /tmp 里会被系统清理器顺手删掉，表现为「刚列出来的 commit 忽然取不到」。
    //
    // 两级结构**跨重启**（决策 257）：`config.toml` 那一级进 `configured_repos`，界面保存过的
    // 那一级从 DB 读回并经 `set_market_override` 装上——与 `server_bind_override` 同构
    // （那个也是启动时读、界面写的，因为绑地址必须在启动时解析）。**`.await` 不能省**：
    // 少了这一步，界面保存的名单在重启后静默回落到 `config.toml`，而「显式清空」与
    // 「没保存过」也就再也分不开——那正是存储层 doc（`storage/market_repos.rs`）与
    // `glossary.md`「技能市场」条明写不许丢的那条区分。
    let market_repos = config.market.resolved_repos();
    let market_override = store.market_repos_override().await?;
    let repo = Arc::new(Libgit2Repo::new(home.root().join("market-repos")));
    // 值班长（决策 182）：与执行器共用同一个 LLM 出口。构造在 `AppState::new` 之前
    // ——那一步会消费掉 store / home / settings。
    let foreman = Arc::new(
        ForemanRunner::new(
            store.clone(),
            settings.clone(),
            home.clone(),
            runtime.llm(),
            sse.clone(),
        )
        // 托管放行的自动动作（决策 210② / 票 08、票 09）：resume 与 unstick 各走各自那份实现。
        .with_steward_actions(Arc::new(crate::runtime::StewardActions::new(
            store.clone(),
            settings.clone(),
            runtime.resume_hook.clone(),
            sse.clone(),
        ))),
    );
    // 值守轮（决策 209④ / 票 06）：与调度器的 10s tick 分开驱动——它要花模型的钱，
    // 且「醒不醒」的判据在 `ForemanRunner::watch` 里（有待办 + 去抖窗口到期）。
    runtime.spawn_watch_loop(foreman.clone(), shutdown_tx.subscribe());
    let state = AppState::new(store, home, settings, bound.port())
        .with_sse(sse)
        .with_executor(runtime.executor())
        .with_resume_hook(runtime.resume_hook.clone())
        .with_bind_host(host.clone())
        .with_bind_source(bind_source)
        .with_port_source(port_source)
        .with_rebind(rebind_tx)
        .with_allowed_origins(extra_origins)
        .with_repo(repo, market_repos)
        .with_market_override(market_override)
        .with_notify_config(config.notify.clone())
        .with_foreman(foreman);
    let router = build_router(state.clone());

    tracing::info!(%bound, port = bound.port(), host_source = bind_source.as_str(), "AgentPipeline 已启动");
    // 就绪标记（决策 153⑤）：tracing 输出受 `RUST_LOG` 过滤，子进程（冒烟测试 / 桌面壳）
    // 需要一条不受日志级别影响的确定性信号来读取内核分配的真实端口。
    println!("AGENTPIPELINE_READY port={}", bound.port());

    // 首个监听器直接把已经绑好的 listener 交出去（不再二次 bind：那会在
    // 「先 bind 再交给 serve」之间留一个端口被别的进程抢走的窗口）。
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let current = Listener {
        stop: stop_tx,
        task: tokio::spawn(async move {
            axum::serve(listener, into_serving_service(router))
                .with_graceful_shutdown(async move {
                    let _ = stop_rx.changed().await;
                })
                .await
                .context("axum 服务异常退出")
                .map_err(|e| tracing::error!(error = %e, "监听任务退出"))
                .ok();
        }),
    };

    // 监听器主管（决策 186）：串行处理改绑请求，停机时随全局 shutdown 一起收摊。
    let supervisor = tokio::spawn(run_listener_supervisor(
        ListenerSupervisor {
            state: state.clone(),
            current: Some(current),
            port: actual_port,
            config_host: server.host.clone(),
            startup_override: options.host_override.clone(),
        },
        rebind_rx,
        shutdown_tx.subscribe(),
    ));

    Ok(ServerHandle {
        port: bound.port(),
        shutdown: shutdown_tx,
        server: supervisor,
    })
}

/// 初始化 tracing（§10.6.5 的 `[logging] level` / `format` / `file`，票 16）。
///
/// - `level`：`EnvFilter` 表达式，非法时回退 `info`（不阻断启动）；
/// - `format`：`pretty`（缺省）/ `compact` / `json`；旧键 `json_file = true` 等价 `json`；
/// - `file`：日志文件的落点（`~` 可展开、相对路径按 home 根解析）；目录自动创建并
///   收紧到 0700、文件 0600（§12.14）。**未配置时缺省落
///   `{home}/logs/agentpipeline.log`**（`LoggingConfig::resolved_file`）——`logs/` 是家目录
///   骨架里就建好的那个目录（`Home::ensure_dirs`），缺省不落文件等于让它永远空着；
///   而桌面壳从 Finder 启动时没有终端，标准输出哪儿都不去，日志便等于不存在。
///   文件无法创建时**不**阻断启动，改为把原因打到标准错误——日志初始化失败不该让
///   服务起不来。
pub fn init_tracing(config: &Config, home: &Home) {
    let subscriber = build_subscriber(config, home);
    // 已注册（测试进程里的第二次调用）不视为错误。
    if tracing::subscriber::set_global_default(subscriber).is_ok() {
        tracing::info!(
            level = %config.logging.level,
            format = config.logging.effective_format().as_str(),
            file = ?config.logging.resolved_file(home.root()),
            "日志已初始化"
        );
    }
}

/// 按 `[logging]` 构造 subscriber（与全局注册分离，便于测试用 `with_default` 捕获）。
///
/// **标准输出与日志文件各成一层**：着色是**每层**自己的属性，两条输出共用一个 writer 时
/// 只能整体二选一——配了 `file` 就连终端一起失去颜色（此前正是如此）。分层之后，终端那层
/// 照旧着色，文件那层恒不着色，转义码不会污染日志文件。
fn build_subscriber(config: &Config, home: &Home) -> Box<dyn tracing::Subscriber + Send + Sync> {
    use tracing_subscriber::{fmt, layer::SubscriberExt as _, EnvFilter, Layer as _};

    // filter 每层各带一份：「哪些事件能出去」与「层怎么排」无关，也就不必记全局过滤器的顺序约定。
    let filter =
        || EnvFilter::try_new(&config.logging.level).unwrap_or_else(|_| EnvFilter::new("info"));
    let format = config.logging.effective_format();
    // 只有 pretty 着色：json / compact 本就不该带颜色
    let ansi = matches!(format, LogFormat::Pretty);

    // 文件日志：目录 + 文件（打不开则降级为仅标准输出，不阻断启动）
    let file = match config.logging.resolved_file(home.root()) {
        Some(path) => match open_log_file(&path) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("警告：日志文件不可用，本次仅写标准输出：{e:#}");
                None
            }
        },
        None => None,
    };

    // 决策 321：慢语句的存储水位伴随层（WAL / 磁盘剩余），三种格式共用一份观察。
    let io_budget_layer =
        crate::io_budget::SlowStatementLayer::new(home.db_path(), home.data_dir());

    // 决策 321：观察层**最后注册**——分发按注册顺序，慢语句的现场伴随行必须排在
    // 它所依附的那行 sqlx 告警**之后**（先输出、后观察，才不会把现场排到引用它的
    // 那行前面）。
    match format {
        LogFormat::Json => Box::new(
            tracing_subscriber::registry()
                .with(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_ansi(ansi)
                        .event_format(JsonEvent)
                        .with_filter(filter()),
                )
                .with(file.map(|f| {
                    fmt::layer()
                        .with_writer(f)
                        .with_ansi(false)
                        .event_format(JsonEvent)
                        .with_filter(filter())
                }))
                .with(io_budget_layer),
        ),
        LogFormat::Compact => Box::new(
            tracing_subscriber::registry()
                .with(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_ansi(ansi)
                        .compact()
                        .with_filter(filter()),
                )
                .with(file.map(|f| {
                    fmt::layer()
                        .with_writer(f)
                        .with_ansi(false)
                        .compact()
                        .with_filter(filter())
                }))
                .with(io_budget_layer),
        ),
        LogFormat::Pretty => Box::new(
            tracing_subscriber::registry()
                .with(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_ansi(ansi)
                        .with_filter(filter()),
                )
                .with(file.map(|f| {
                    fmt::layer()
                        .with_writer(f)
                        .with_ansi(false)
                        .with_filter(filter())
                }))
                .with(io_budget_layer),
        ),
    }
}

/// 每行一条 JSON 的事件格式化器（`format = "json"`，票 16）。
///
/// 不引入 `tracing-subscriber/json`（那需要新增依赖与 feature），只用 serde_json
/// 渲染 `timestamp` / `level` / `target` 与事件字段；`message` 为普通字段一并输出。
struct JsonEvent;

impl<S, N> tracing_subscriber::fmt::format::FormatEvent<S, N> for JsonEvent
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::format::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        use serde_json::Value;
        let meta = event.metadata();
        let mut map = serde_json::Map::new();
        map.insert(
            "timestamp".into(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
        map.insert("level".into(), Value::String(meta.level().to_string()));
        map.insert("target".into(), Value::String(meta.target().to_string()));
        event.record(&mut JsonVisitor { map: &mut map });
        let line = Value::Object(map);
        writeln!(writer, "{line}")
    }
}

/// 把事件字段按类型收集进 JSON 对象；未知类型退化为 `Debug` 字符串。
struct JsonVisitor<'a> {
    map: &'a mut serde_json::Map<String, serde_json::Value>,
}

impl tracing::field::Visit for JsonVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.map.insert(
            field.name().to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.map.insert(
            field.name().to_string(),
            serde_json::Value::String(format!("{value:?}")),
        );
    }
}

/// 创建日志文件所在目录（仅新建时收紧 0700）、打开文件并收紧为 0600（§12.14）。
///
/// 只对**本次新建**的目录收紧权限：用户可能把 `file` 指到已有共享目录（如 `/var/log`），
/// 对其 chmod 会造成破坏。
fn open_log_file(path: &std::path::Path) -> anyhow::Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let created = !dir.exists();
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建日志目录失败：{}", dir.display()))?;
            if created {
                restrict_permissions(dir);
            }
        }
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("创建日志文件失败：{}", path.display()))?;
    restrict_file_permissions(path);
    Ok(file)
}

/// 准备 `[prompts] dir` 覆盖目录：不存在则创建；仅新建时收紧 0700（§12.14）。
///
/// 尽力而为：创建失败只告警——目录缺失时 `resolve_persona` 本就会回落内嵌 persona，
/// 不该因覆盖目录建不出来而拒绝启动。
///
/// 技能根（`[skills] dir`）**不走这里**：它常指向用户已有的技能生态目录
/// （`~/.zcode/skills`），新建或改权限都不是本系统该做的事（决策 172）；技能根不存在
/// 只是「没有可用技能」，无需预备。
fn prepare_prompts_dir(dir: &std::path::Path) {
    let created = !dir.exists();
    match std::fs::create_dir_all(dir) {
        Ok(()) => {
            if created {
                restrict_permissions(dir);
            }
        }
        Err(e) => eprintln!("警告：创建 prompts 覆盖目录 {} 失败：{e}", dir.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binding_port_zero_reads_back_kernel_assigned_port() {
        // 决策 153⑤：绑定 0 时返回的必须是内核分配的真实端口，不能是 0。
        let (listener, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        assert_ne!(bound.port(), 0, "应回读内核分配的真实端口");
        assert_eq!(listener.local_addr().unwrap().port(), bound.port());
    }

    #[test]
    fn startup_seeds_factory_defaults() {
        // 决策 261 的**接线守卫**：出厂播种必须真的发生在启动路径上。判据是读本文件的
        // **生产段**源码——先把 `#[cfg(test)]` 之后的测试段切掉再找：测试段自身就含
        // 这个字面量（本断言就在里面），不切的话删掉生产调用这把尺也照样绿（自证式
        // 守卫，code-review 抓出来的）。serve() 本体没法不绑端口地单测，接线只能这样钉。
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/serve.rs"));
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("serve.rs 含测试段");
        assert!(
            production.contains("factory::seed_factory_defaults("),
            "serve() 的生产段必须调用出厂播种（决策 261）"
        );
    }

    #[test]
    fn serving_service_carries_connect_info() {
        // 类型即断言（票 06）：只有 IntoMakeServiceWithConnectInfo 能绑到这个类型上，
        // 裸 Router 交进来会在编译期被拒——对端地址正是靠这个返回类型才活到处理器。
        use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
        let _serving: IntoMakeServiceWithConnectInfo<axum::Router, std::net::SocketAddr> =
            into_serving_service(axum::Router::new());
    }

    // ── 决策 186：绑定地址的三级来源 ──

    #[test]
    fn startup_override_beats_settings_and_config() {
        let (host, source) = resolve_bind_host(
            Some("10.0.0.5".to_string()),
            Some("0.0.0.0".to_string()),
            "127.0.0.1",
        );
        assert_eq!(host, "10.0.0.5");
        assert_eq!(source, crate::state::BindSource::Startup);
    }

    #[test]
    fn settings_beat_config_but_not_startup() {
        let (host, source) = resolve_bind_host(None, Some("0.0.0.0".to_string()), "127.0.0.1");
        assert_eq!(host, "0.0.0.0");
        assert_eq!(source, crate::state::BindSource::Settings);
    }

    #[test]
    fn config_is_the_fallback_and_is_reported_as_such() {
        let (host, source) = resolve_bind_host(None, None, "192.168.1.10");
        assert_eq!(host, "192.168.1.10");
        assert_eq!(source, crate::state::BindSource::Config);
        // 缺省配置也是这一级（分享页据此显示「这是配置文件里的值」）
        let (host, source) = resolve_bind_host(None, None, "127.0.0.1");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(source, crate::state::BindSource::Config);
    }

    #[test]
    fn bind_source_strings_are_the_api_contract() {
        // 这三个串是 `/server-info.bind_source` 的取值（前端据此选文案），改名即改契约。
        assert_eq!(crate::state::BindSource::Startup.as_str(), "startup");
        assert_eq!(crate::state::BindSource::Settings.as_str(), "settings");
        assert_eq!(crate::state::BindSource::Config.as_str(), "config");
    }

    #[tokio::test]
    async fn binding_occupied_port_reports_clear_error() {
        // 端口占用仍须明确报错（不静默换端口）。
        let (listener, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        let err = bind_listener("127.0.0.1", bound.port())
            .await
            .expect_err("同端口二次绑定应失败");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("端口被占用或无法绑定"),
            "错误信息应明确：{msg}"
        );
        drop(listener);
    }

    // ── 决策 213：首选端口 + 「占用则退让」 ──

    /// 拿一个空闲端口当「首选」：先绑 `:0` 读出号码再释放。
    ///
    /// 这是**测试夹具**的手法；生产侧不做这种探测（决策 153⑤ 明写不再需要
    /// 「先探测空闲端口再释放」，那中间有一个别人抢走的窗口）。
    async fn free_port() -> u16 {
        let (probe, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        let port = bound.port();
        drop(probe);
        port
    }

    #[tokio::test]
    async fn preferred_port_is_used_when_free() {
        use crate::state::PortSource;
        let port = free_port().await;
        let (listener, bound, source) =
            bind_preferred("127.0.0.1", port, PortSource::Config, false)
                .await
                .unwrap();
        assert_eq!(
            bound.port(),
            port,
            "首选端口空闲时必须用它（这正是不换端口的保证）"
        );
        assert_eq!(source, PortSource::Config, "没退让就不该改来源");
        drop(listener);
    }

    #[tokio::test]
    async fn occupied_preferred_port_falls_back_when_allowed() {
        use crate::state::PortSource;
        // 占住一个端口，再让它当首选：桌面壳那条路（打不开窗比端口变一次更糟）必须能起。
        let (held, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        let (listener, fallback, source) =
            bind_preferred("127.0.0.1", bound.port(), PortSource::Config, true)
                .await
                .unwrap();
        assert_ne!(fallback.port(), bound.port(), "首选被占时应换一个端口");
        assert_ne!(fallback.port(), 0, "回读的必须是真实端口");
        assert_eq!(
            source,
            PortSource::Fallback,
            "退让必须被标记出来：分享页据此说明「手机上的旧网址这次失效了」"
        );
        drop(listener);
        drop(held);
    }

    #[tokio::test]
    async fn occupied_preferred_port_is_fatal_without_fallback() {
        use crate::state::PortSource;
        // 缺省姿态（命令行 / 测试）：显式指定的端口绑不上就报错，不悄悄换一个。
        let (held, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        let err = bind_preferred("127.0.0.1", bound.port(), PortSource::Startup, false)
            .await
            .expect_err("未打开退让时端口占用应报错");
        let msg = format!("{err:#}");
        assert!(msg.contains("端口被占用或无法绑定"), "{msg}");
        assert!(is_addr_in_use(&err), "占用必须被识别成 AddrInUse");
        drop(held);
    }

    #[test]
    fn fallback_is_opt_in() {
        // 缺省不开：`--port` 是用户明确指定的，绑不上就该看见错误（决策 213）。
        assert!(
            !ServeOptions::default().port_fallback_to_ephemeral,
            "退让只能是显式开的（桌面壳）——缺省打开会把「端口被占」这条唯一不需要猜的错误吃掉"
        );
    }

    #[test]
    fn only_addr_in_use_is_recognized_as_occupied() {
        // 按**错误链**判而不是按报文判：`bind_listener` 的上下文同时罩着权限 / 地址不可用
        // 等不该退让的失败，按报文判会让它们静默退让到随机端口。
        let other =
            anyhow::anyhow!("permission denied").context("端口被占用或无法绑定：127.0.0.1:80");
        assert!(!is_addr_in_use(&other), "别的绑定失败不得被当成端口占用");
        let io = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        assert!(is_addr_in_use(
            &anyhow::Error::from(io).context("任何上下文")
        ));
    }

    #[test]
    fn port_source_strings_are_the_api_contract() {
        // 这三个串是 `/server-info.port_source` 的取值（前端据此选文案），改名即改契约。
        use crate::state::PortSource;
        assert_eq!(PortSource::Startup.as_str(), "startup");
        assert_eq!(PortSource::Config.as_str(), "config");
        assert_eq!(PortSource::Fallback.as_str(), "fallback");
    }

    // ── 票 16：`[logging] format / file` 真正生效 ──

    fn logging_config(toml: &str) -> Config {
        Config::from_toml(toml).unwrap()
    }

    /// 缺省（没写 `[logging] file`）就要落文件——这是那个「日志没输出到日志文件」的
    /// 真问题：`config.toml` 是可选的，没人写它就一直没有日志文件，而桌面壳从 Finder
    /// 启动时连标准输出都没有。
    #[test]
    fn log_file_is_written_even_without_explicit_file_config() {
        let home = tempfile::tempdir().unwrap();
        let home = Home::new(home.path());
        // 一个字都不配：全缺省
        let cfg = logging_config("");

        let sub = build_subscriber(&cfg, &home);
        tracing::subscriber::with_default(sub, || {
            tracing::info!("缺省也要落文件");
        });

        let path = home.root().join("logs/agentpipeline.log");
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("缺省应创建日志文件 {}：{e}", path.display()));
        assert!(content.contains("缺省也要落文件"), "内容：{content}");
    }

    /// 决策 231 的三行日志：**节点开始 / 请求派发 / 请求收场**，按时间顺序落在同一个文件里。
    ///
    /// 为什么要有这一条：「日志读得到」（决策 226）与「日志里有东西」是两件事——09-19 的
    /// 实测里值班长自己报出「这份日志只有 16 行、没有任何一条逐 run / 逐节点的行」，于是
    /// 「零 token」在那份证据面上无解。这三行就是给那份证据面补的**时间线**（表回答「现在
    /// 在飞什么」，日志回答「那一刻按顺序发生了什么」，决策 231 两条都要）。
    #[test]
    fn the_three_model_log_lines_form_a_timeline_in_the_file() {
        use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest};
        use agentpipeline_core::storage::observability::NewProjectRun;
        use agentpipeline_core::types::{Node, Stage};

        /// 当场返回的替身；本用例只关心日志，不关心读数。
        struct Stub;
        impl LlmClient for Stub {
            fn complete(
                &self,
                _request: LlmRequest,
            ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>>
            {
                Box::pin(async { Ok(AgentResponse::default()) })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        let cfg = logging_config("[logging]\nlevel = \"info\"\n");
        let sub = build_subscriber(&cfg, &home);

        // `with_default` 是**线程局部**的，而 `current_thread` 运行时就在本线程上跑，
        // 故这三行确实过的是上面那个 subscriber（换成多线程运行时这条用例会变成 flaky）。
        tracing::subscriber::with_default(sub, || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let store = Store::open(Home::new(dir.path()), Arc::new(SystemClock))
                    .await
                    .unwrap();
                testkit::seed_project(&store, "p1", "示例", dir.path(), "main")
                    .await
                    .unwrap();
                // 第一行：节点开始（项目级 run 是「没有任务归属」的那一支）。
                store
                    .insert_project_run(&NewProjectRun {
                        project_id: "p1".into(),
                        stage: Stage::Init,
                        node: Node::Execute,
                        attempt: 1,
                        agent_type: "pseudo:project_analysis".into(),
                    })
                    .await
                    .unwrap();
                // 第二、三行：请求派发 / 请求收场。
                let llm =
                    agentpipeline_core::agent::RecordingLlm::new(Arc::new(Stub), store.clone());
                llm.complete(LlmRequest {
                    stage: Stage::Init,
                    node: Node::Execute,
                    attempt: 1,
                    system_prompt: "s".into(),
                    user_prompt: "u".into(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                    temperature: None,
                    max_tokens: None,
                    provider_id: None,
                    run: None,
                    idle_timeout_sec: None,
                })
                .await
                .unwrap();
            });
        });

        let content = std::fs::read_to_string(home.root().join("logs/agentpipeline.log")).unwrap();
        let start = content
            .find("节点开始")
            .unwrap_or_else(|| panic!("第一行缺失（节点开始）—— 日志里没有逐节点的行：{content}"));
        let dispatch = content
            .find("模型请求派发")
            .unwrap_or_else(|| panic!("第二行缺失（请求派发）：{content}"));
        let settle = content
            .find("模型请求收场")
            .unwrap_or_else(|| panic!("第三行缺失（请求收场）：{content}"));
        assert!(
            start < dispatch && dispatch < settle,
            "三行的顺序就是时间线（节点开始 → 请求派发 → 请求收场）：{content}"
        );
        assert!(
            content.contains("pseudo:project_analysis"),
            "节点开始那一行要说清是哪个节点 / 哪种调用：{content}"
        );
    }

    /// `file = ""` 是「只写标准输出」的逃生口（与「没配」分得开）。
    #[test]
    fn blank_file_config_keeps_logs_on_stdout_only() {
        let home = tempfile::tempdir().unwrap();
        let home = Home::new(home.path());
        let cfg = logging_config("[logging]\nfile = \"\"\n");

        let sub = build_subscriber(&cfg, &home);
        tracing::subscriber::with_default(sub, || {
            tracing::info!("只写标准输出");
        });

        assert!(
            !home.root().join("logs/agentpipeline.log").exists(),
            "显式空白不该建日志文件"
        );
    }

    /// 终端那层着色、文件那层不着色：转义码不得污染日志文件（同一个 writer 做不到这件事，
    /// 故两层分开）。
    #[test]
    fn log_file_never_contains_ansi_escapes() {
        let home = tempfile::tempdir().unwrap();
        let home = Home::new(home.path());
        let cfg = logging_config("[logging]\nlevel = \"info\"\n"); // pretty（缺省）= 终端着色那一档

        let sub = build_subscriber(&cfg, &home);
        tracing::subscriber::with_default(sub, || {
            tracing::info!("转义码不该进文件");
        });

        let content = std::fs::read_to_string(home.root().join("logs/agentpipeline.log")).unwrap();
        assert!(content.contains("转义码不该进文件"), "内容：{content}");
        assert!(
            !content.contains('\u{1b}'),
            "文件里出现了 ANSI 转义码：{content:?}"
        );
    }

    #[test]
    fn file_logging_creates_dir_and_file_with_restricted_permissions() {
        let home = tempfile::tempdir().unwrap();
        let cfg =
            logging_config("[logging]\nlevel = \"info\"\nfile = \"logs/agentpipeline.log\"\n");
        let home = Home::new(home.path());

        let sub = build_subscriber(&cfg, &home);
        tracing::subscriber::with_default(sub, || {
            tracing::info!(probe = "file-test", "日志文件落盘验证");
        });

        let path = home.root().join("logs/agentpipeline.log");
        assert!(
            path.is_file(),
            "file 配置应创建日志文件：{}",
            path.display()
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("日志文件落盘验证"), "内容：{content}");
        assert!(content.contains("file-test"), "字段也应落入文件：{content}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "日志文件应为 0600（§12.14）");
            let dir_mode = std::fs::metadata(home.root().join("logs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "日志目录应为 0700（§12.14）");
        }
    }

    #[test]
    fn json_format_writes_one_json_object_per_line() {
        let home = tempfile::tempdir().unwrap();
        let cfg = logging_config(
            "[logging]\nlevel = \"info\"\nformat = \"json\"\nfile = \"logs/ap.log\"\n",
        );
        let home = Home::new(home.path());

        let sub = build_subscriber(&cfg, &home);
        tracing::subscriber::with_default(sub, || {
            tracing::info!(answer = 42, "json line");
        });

        let content = std::fs::read_to_string(home.root().join("logs/ap.log")).unwrap();
        let line = content.lines().find(|l| !l.trim().is_empty()).unwrap();
        let v: serde_json::Value = serde_json::from_str(line).expect("每行应是合法 JSON");
        assert_eq!(v["level"], "INFO");
        assert_eq!(v["message"], "json line");
        assert_eq!(v["answer"], 42);
        assert!(v["timestamp"].is_string());
    }

    #[test]
    fn compact_format_is_single_line_text() {
        let home = tempfile::tempdir().unwrap();
        let cfg = logging_config(
            "[logging]\nlevel = \"info\"\nformat = \"compact\"\nfile = \"logs/ap.log\"\n",
        );
        let home = Home::new(home.path());

        let sub = build_subscriber(&cfg, &home);
        tracing::subscriber::with_default(sub, || {
            tracing::info!("compact line");
        });

        let content = std::fs::read_to_string(home.root().join("logs/ap.log")).unwrap();
        assert!(content.contains("compact line"), "内容：{content}");
        let non_empty: Vec<_> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(non_empty.len(), 1, "compact 每事件一行：{content}");
    }

    #[test]
    fn deprecated_json_file_key_still_drives_json_format() {
        let home = tempfile::tempdir().unwrap();
        let cfg = logging_config("[logging]\nlevel = \"info\"\njson_file = true\n");
        // 旧键在写作路径上被接受（废弃但不静默忽略）
        assert!(cfg.logging.format.is_none());
        assert_eq!(cfg.logging.effective_format(), LogFormat::Json);
        let _ = home;
    }

    #[test]
    fn log_file_failure_does_not_block_startup() {
        // 父路径是文件而非目录 → 目录创建必然失败；初始化仍不得 panic
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        let home = Home::new(blocker);
        let cfg = logging_config("[logging]\nfile = \"logs/x.log\"\n");
        init_tracing(&cfg, &home);
    }

    #[test]
    fn prepare_prompts_dir_creates_override_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("custom-prompts");
        prepare_prompts_dir(&dir);
        assert!(dir.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "新建覆盖目录应为 0700（§12.14）");
        }
    }
}
