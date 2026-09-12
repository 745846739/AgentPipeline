//! 服务启动（决策 153⑤：起服逻辑沉入 lib，供二进制、冒烟测试与桌面壳复用）。
//!
//! 二进制入口只做参数解析后调用 [`serve`]；绑定支持端口 `0`——此时内核分配真实端口，
//! 由 [`ServerHandle::port`] 与启动日志给出，调用方不再需要「先探测空闲端口再释放」
//! 的绕开（决策 153⑤；决策 128 的同源 origin 判定也依赖这个真实端口）。

use std::sync::Arc;

use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::{Config, LogFormat};
use agentpipeline_core::home::{
    check_permissions, restrict_file_permissions, restrict_permissions, Home,
};
use agentpipeline_core::sse::SseBus;
use agentpipeline_core::storage::Store;
use anyhow::Context;
use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt};

use crate::runtime::Runtime;
use crate::{build_router, AppState};

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

/// 以给定配置启动服务，返回可读回真实端口的句柄。
///
/// `port_override` 为 `None` 时回落 `[server] port`（§10.6.5）；`0` 表示由内核分配，
/// 真实端口经 [`ServerHandle::port`] 与启动日志给出（决策 153⑤）。
pub async fn serve(port_override: Option<u16>) -> anyhow::Result<ServerHandle> {
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
    // 文件目录在此创建并收紧权限（0700 / 0600，§12.14）。
    init_tracing(&config, &home)?;

    // `[prompts] dir` 覆盖接入（票 16）：executor 经 `Home::prompts_dir` 取模板目录，
    // 覆盖目录不存在时也照常回落内嵌 persona（决策 7）。
    let home = match config.prompts.resolved_dir(home.root()) {
        Some(dir) => {
            prepare_prompts_dir(&dir)?;
            home.with_prompts_dir(Some(dir))
        }
        None => home,
    };

    let store = Store::open(home.clone(), Arc::new(SystemClock)).await?;

    // 恢复流程第一步（决策 127）：清理 kill -9 残留的 executor_owner
    let cleared = store.clear_executor_owners().await?;
    if cleared > 0 {
        tracing::info!(cleared, "已清理残留的 executor 持有者");
    }

    // 配置 fail fast（决策 47 / 103 / 134）
    let report = store.validate_startup(&settings).await?;
    if !report.demoted_providers.is_empty() {
        tracing::warn!(?report.demoted_providers, "不受支持的 vendor 已降级 enabled=0");
    }

    // CLI --port > [server] port（§10.6.5 的配置此前被硬编码架空，决策 128 修订同批对齐）
    let server = config.server.clone();
    let port = port_override.unwrap_or(server.port);

    // 停机信号（决策 54）：一处广播，三处消费——axum、tick 循环、维护循环。
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    // 生产运行时（票 17）：真实 LLM + executor + scheduler（决策 55）。
    let sse = Arc::new(SseBus::default());
    let runtime = Runtime::new(store.clone(), settings.clone(), sse.clone());
    runtime.spawn_tick_loop(store.clone(), settings.clone(), shutdown_tx.subscribe());
    runtime.spawn_maintenance_loop(store.clone(), settings.clone(), shutdown_tx.subscribe());

    // 先绑定再建 state：端口 0 时把内核分配的真实端口交给 AppState，
    // 决策 128 的本机 origin 白名单必须用真实端口（用 0 会拒掉桌面壳的同源请求）。
    let (listener, bound) = bind_listener(&server.host, port).await?;

    let state = AppState::new(store, home, settings, bound.port())
        .with_sse(sse)
        .with_executor(runtime.executor())
        .with_resume_hook(runtime.resume_hook.clone());
    let router = build_router(state);

    tracing::info!(%bound, port = bound.port(), "AgentPipeline 已启动");
    // 就绪标记（决策 153⑤）：tracing 输出受 `RUST_LOG` 过滤，子进程（冒烟测试 / 桌面壳）
    // 需要一条不受日志级别影响的确定性信号来读取内核分配的真实端口。
    println!("AGENTPIPELINE_READY port={}", bound.port());

    let server_task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
            .context("axum 服务异常退出")
    });

    Ok(ServerHandle {
        port: bound.port(),
        shutdown: shutdown_tx,
        server: server_task,
    })
}

/// 初始化 tracing（§10.6.5 的 `[logging] level` / `format` / `file`，票 16）。
///
/// - `level`：`EnvFilter` 表达式，非法时回退 `info`（不阻断启动）；
/// - `format`：`pretty`（缺省）/ `compact` / `json`；旧键 `json_file = true` 等价 `json`；
/// - `file`：同时写入该文件（`~` 可展开、相对路径按 home 根解析）；目录自动创建并
///   收紧到 0700、文件 0600（§12.14）。文件无法创建时**不**阻断启动，改为把原因
///   打到标准错误——日志初始化失败不该让服务起不来。
pub fn init_tracing(config: &Config, home: &Home) -> anyhow::Result<()> {
    let subscriber = build_subscriber(config, home);
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(())
}

/// 按 `[logging]` 构造 subscriber（与全局注册分离，便于测试用 `with_default` 捕获）。
fn build_subscriber(config: &Config, home: &Home) -> Box<dyn tracing::Subscriber + Send + Sync> {
    use tracing_subscriber::EnvFilter;

    let filter =
        EnvFilter::try_new(&config.logging.level).unwrap_or_else(|_| EnvFilter::new("info"));
    let format = config.logging.effective_format();

    // 文件日志：目录 + 文件（打不开则降级为仅标准输出，不阻断启动）
    let (writer, has_file): (BoxMakeWriter, bool) = match config.logging.resolved_file(home.root())
    {
        Some(path) => match open_log_file(&path) {
            Ok(file) => (BoxMakeWriter::new(std::io::stdout.and(file)), true),
            Err(e) => {
                eprintln!("警告：日志文件不可用，本次仅写标准输出：{e:#}");
                (BoxMakeWriter::new(std::io::stdout), false)
            }
        },
        None => (BoxMakeWriter::new(std::io::stdout), false),
    };

    // 写文件时禁 ANSI，避免转义码污染 JSON / 日志文件；json / compact 也不需要颜色
    let ansi = matches!(format, LogFormat::Pretty) && !has_file;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(ansi);

    match format {
        LogFormat::Json => Box::new(
            builder
                .event_format(JsonEvent)
                .fmt_fields(tracing_subscriber::fmt::format::DefaultFields::new())
                .finish(),
        ),
        LogFormat::Compact => Box::new(builder.compact().finish()),
        LogFormat::Pretty => Box::new(builder.finish()),
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
fn prepare_prompts_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    let created = !dir.exists();
    std::fs::create_dir_all(dir)
        .with_context(|| format!("创建 prompts 目录失败：{}", dir.display()))?;
    if created {
        restrict_permissions(dir);
    }
    Ok(())
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

    // ── 票 16：`[logging] format / file` 真正生效 ──

    fn logging_config(toml: &str) -> Config {
        Config::from_toml(toml).unwrap()
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
        // 父路径是文件而非目录 → 目录创建必然失败；初始化仍不得 panic / 报错
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        let home = Home::new(blocker);
        let cfg = logging_config("[logging]\nfile = \"logs/x.log\"\n");
        init_tracing(&cfg, &home).unwrap();
    }

    #[test]
    fn prepare_prompts_dir_creates_override_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("custom-prompts");
        prepare_prompts_dir(&dir).unwrap();
        assert!(dir.is_dir());
    }
}
