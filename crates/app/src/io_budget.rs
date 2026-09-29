//! 慢语句的现场快照层（决策 321）。
//!
//! sqlx 自己会打 `slow statement` 的 WARN（`sqlx::query` target），但那行只有
//! SQL 与耗时——2026-09 排查时发现「是不是磁盘满了 / WAL 是不是涨大了」这类
//! 一眼定性的问题它答不了，只能靠外部猜。本层订阅同一个事件流，对**每一条**
//! 慢语句追加一行 `storage::io_budget` 的伴随告警，把 WAL 字节数与磁盘剩余
//! 空间钉在同一份现场里。
//!
//! 设计约束：
//! - **只观察、不转发**：本层不拦截也不修改 sqlx 的原始事件，只是再发一条自己的；
//!   自己发的事件 target 是 `storage::io_budget`，`on_event` 里按 target 闸死，
//!   不会递归。
//! - **读数失败照发**：WAL 读不到、statvfs 失败时水位字段记缺省值并带
//!   `*_present = false` 的口供，告警本身不缺席——可观测性不许把主流程带下水，
//!   也不许因为读不到水位就丢掉告警。
//! - 阈值取 sqlx 的 `slow_threshold` 缺省（1s），只对慢语句付 statvfs 的成本。

use std::path::PathBuf;

use agentpipeline_core::storage::io_budget;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// 从 sqlx 事件里取得到的字段（`summary` / `elapsed_secs`）。
#[derive(Debug, Default)]
pub(crate) struct SlowStatementFields {
    pub summary: Option<String>,
    pub elapsed_secs: Option<f64>,
}

impl SlowStatementFields {
    /// 这条事件够不够格触发伴随告警。
    fn is_slow(&self, threshold_secs: f64) -> bool {
        self.elapsed_secs.is_some_and(|e| e >= threshold_secs)
    }
}

impl Visit for SlowStatementFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "summary" {
            self.summary = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "summary" {
            self.summary = Some(format!("{value:?}"));
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.elapsed_secs = Some(value);
        }
    }
}

/// target 闸门：只听 sqlx 的慢语句事件，且不理会自己发出去的那条（防递归）。
fn is_slow_statement(target: &str) -> bool {
    target == "sqlx::query"
}

/// 慢语句 → 伴随 `storage::io_budget` 告警的观察层。
pub struct SlowStatementLayer {
    db_path: PathBuf,
    data_dir: PathBuf,
    /// 只对超过这个耗时的语句付水位读数的成本（秒；sqlx 缺省 slow_threshold = 1s）。
    threshold_secs: f64,
}

impl SlowStatementLayer {
    pub fn new(db_path: PathBuf, data_dir: PathBuf) -> Self {
        SlowStatementLayer {
            db_path,
            data_dir,
            threshold_secs: 1.0,
        }
    }
}

impl<S> Layer<S> for SlowStatementLayer
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if !is_slow_statement(event.metadata().target()) {
            return;
        }
        let mut fields = SlowStatementFields::default();
        event.record(&mut fields);
        if !fields.is_slow(self.threshold_secs) {
            return;
        }
        let wal_bytes = io_budget::wal_bytes(&self.db_path);
        let disk_free_bytes = io_budget::disk_free_bytes(&self.data_dir);
        tracing::warn!(
            target: "storage::io_budget",
            summary = fields.summary.as_deref().unwrap_or("?"),
            elapsed_secs = fields.elapsed_secs.unwrap_or_default(),
            wal_bytes = wal_bytes.unwrap_or(0),
            wal_bytes_present = wal_bytes.is_some(),
            disk_free_bytes = disk_free_bytes.unwrap_or(0),
            disk_free_present = disk_free_bytes.is_some(),
            "慢语句现场：存储水位快照（WAL / 磁盘剩余）"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_gate_only_listens_to_sqlx_query() {
        assert!(is_slow_statement("sqlx::query"));
        assert!(
            !is_slow_statement("storage::io_budget"),
            "自己的事件不许再进来（防递归）"
        );
        assert!(!is_slow_statement("app::serve"));
    }

    #[tokio::test]
    async fn layer_emits_companion_warning_with_watermarks() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("x.db");
        std::fs::write(&db, b"0123456789").unwrap();
        std::fs::write(dir.path().join("x.db-wal"), b"walbytes").unwrap();

        let shared: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let sink = shared.clone();
        // 与 build_subscriber 同一形状：输出层在前、观察层最后——分发按注册顺序，
        // 伴随行必须排在它所依附的慢语句行**之后**。
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(move || SinkWriter(sink.clone()))
                    .with_filter(tracing_subscriber::EnvFilter::new("warn")),
            )
            .with(SlowStatementLayer::new(
                db.clone(),
                dir.path().to_path_buf(),
            ));
        // 与生产同路（init_tracing 走 set_global_default）：scoped 的 set_default
        // 带重入守卫，分发期间嵌套发事件会落到 no-op dispatcher——那会把断言带进
        // 与生产不一致的假象里。
        let dispatch = tracing::dispatcher::Dispatch::new(subscriber);
        tracing::dispatcher::set_global_default(dispatch)
            .expect("app lib 测试进程内只应有一个全局 subscriber");

        tracing::warn!(
            target: "sqlx::query",
            summary = "\"DELETE FROM x\"",
            elapsed = tracing::field::debug(std::time::Duration::from_secs(2)),
            elapsed_secs = 2.0,
            "slow statement: execution time exceeded alert threshold"
        );

        let lines = strip_ansi(&String::from_utf8(shared.lock().unwrap().clone()).unwrap());
        let original_at = lines
            .find("slow statement: execution time exceeded")
            .expect("sqlx 原始告警该在");
        let companion_at = lines
            .find("WAL / 磁盘剩余")
            .unwrap_or_else(|| panic!("伴随行没出去，实际输出：{lines}"));
        assert!(
            companion_at > original_at,
            "伴随行排到了原行前面（顺序反了）：{lines}"
        );
        assert!(
            lines.contains("wal_bytes=8"),
            "WAL 字节数没进现场行：{lines}"
        );
        assert!(lines.contains("elapsed_secs=2"), "耗时没进现场行：{lines}");
        assert!(
            lines.contains("DELETE FROM x"),
            "summary 没进现场行：{lines}"
        );
        assert!(
            lines.contains("disk_free_present=true"),
            "磁盘水位该是真读数：{lines}"
        );

        // 快语句（0.02s < 阈值）不追加现场行——statvfs 只为慢语句付钱。
        shared.lock().unwrap().clear();
        tracing::warn!(
            target: "sqlx::query",
            summary = "SELECT fast",
            elapsed_secs = 0.02,
            "slow statement: execution time exceeded alert threshold"
        );
        let after = strip_ansi(&String::from_utf8(shared.lock().unwrap().clone()).unwrap());
        assert!(
            !after.contains("WAL / 磁盘剩余"),
            "快语句不该触发伴随行，实际输出：{after}"
        );
    }

    /// fmt 层的 writer：把每次写入攒进共享缓冲，测试末尾一次性断言。
    #[derive(Clone)]
    struct SinkWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SinkWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SinkWriter {
        type Writer = SinkWriter;

        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    /// 剥掉 ANSI 转义：默认 fmt 层带色，断言字段时不该被转义码打断。
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
