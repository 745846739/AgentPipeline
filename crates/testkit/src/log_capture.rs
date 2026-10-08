//! 日志流捕获（决策 406 起共用）：把 tracing 输出收进内存缓冲，供「这条日志必须存在」
//! 类断言使用。
//!
//! 两个细节是踩出来的，别删：
//!
//! - `#[tokio::test]` 是 current-thread 运行时，[`tracing::subscriber::set_default`] 的
//!   **线程局部**订阅者对 await 期间的 event 同样生效——这是捕获能工作的前提；
//! - **callsite interest 缓存竞态**（票 106-stability/10 的实证）：本进程没有全局订阅者
//!   时，某个日志调用点的**首次发射**若恰好落在「没有 scoped 订阅者」的线程 / 时刻，
//!   interest 会被缓存成 `never`，此后**全进程**在该调用点的事件在宏层被静默丢弃，直到
//!   下次重建。实测签名：两条「工具调用开始」在缓冲里、「收场」零条（2026-10-04 106
//!   merge 闸门实红 + 本机 40 轮 15 红；同窗口的另一个订阅者用例却绿）。
//!
//! 故每个测试二进制在第一次捕获前会自动装一个 writer 为 sink 的**全局兜底**订阅者：
//! 全局注册者一旦存在，所有 callsite 的注册与重建都只会算出 `always`，注册竞态从此
//! 不可达；而派发侧 scoped 订阅者仍然优先（`get_default` 先看线程局部），捕获语义不变。

use std::sync::{Arc, Mutex};

/// 给 tracing 的 callsite interest cache 一个全局兜底（进程内只装一次）。
///
/// [`LogCapture::start`] 会自动调它；单独调只在你需要「先发射、后捕获」的构造里。
pub fn warm_interest_cache() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::INFO)
                .with_ansi(false)
                .with_writer(std::io::sink)
                .finish(),
        );
    });
}

#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 一个**活着的**捕获器：存活期间本线程的 tracing 事件同时进缓冲。
///
/// 销毁（drop）即停止捕获——scoped 订阅者随 guard 一起撤走。
pub struct LogCapture {
    buf: LogBuf,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    /// 开始捕获（INFO 级、无 ANSI 转义，便于子串断言）。
    pub fn start() -> Self {
        warm_interest_cache();
        let buf = LogBuf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        Self { buf, _guard: guard }
    }

    /// 此刻已捕获的全部日志文本。
    pub fn text(&self) -> String {
        String::from_utf8(self.buf.0.lock().unwrap().clone()).unwrap()
    }
}
