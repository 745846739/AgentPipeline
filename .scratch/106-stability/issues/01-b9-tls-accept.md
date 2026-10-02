# 01: TLS accept 并发化 + 握手 10 秒超时——半开连接不得扣住整个监听(B9)

**来源:** 106 生产事故(2026-10-02):Web 界面三次整体打不开,进程活着、端口在听、
主线程 0% CPU 卡在 futex_wait、listen backlog 积压 21 个连接,执行体照常干活。
时间线:重启后 3~15 分钟挂死一次、纯运行中 26~43 分钟挂死一次、再跑 20 分钟挂死一次。
完整证据:`.scratch/monitor-01M3X472FJF8NW9082K6BFZPKC.md` 第六~十三次巡检。

**根因:** `crates/app/src/serve.rs` 的 `TlsListener::accept`(决策 335 手写的 TLS 层)
把 TLS 握手 await 在**唯一的 accept 任务里**且无超时——公网扫描器建 TCP 不发
ClientHello(半开连接),一次握手永久挂住 → accept 停摆 → backlog 积压 → 全站超时。
执行体在别的 task 里不受影响,所以死得悄无声息、没有任何日志。

**Blocked by:** None

**Status:** done

- [x] 握手下放独立任务:`tokio::spawn` + `timeout(TLS_HANDSHAKE_TIMEOUT=10s)` 包住
      `acceptor.accept`;超时/失败只烧那个任务,accept 循环永不等待
- [x] `accept()` 改为 `select!`(biased:先交付已完成握手,再收新 TCP)在
      「完成队列 recv」与「TCP accept」之间轮转
- [x] slowloris 回归测试 `a_silent_tcp_peer_cannot_stall_tls_accept`:半开连接先到、
      真客户端(内嵌自签证书、不验签)后到,断言真客户端 5s 内握手完成、accept 正常
      交出连接、半开连接在超时后被服务端收尸(读到 EOF);旧实现上此测试卡死
- [x] `cargo clippy -D warnings` / fmt 全绿;核心与 app 测试套件全绿

**验收(106 现场公网实测):** 部署后从本机对 `https://106.12.12.6:3389` 发起半开连接
挂 15 秒,断言期间普通 HTTPS 请求仍 200;探测连接约 10 秒被服务端断开。

**边界.** 不换 TLS 栈、不重构 listen 层(最小修,证据只指向握手阻塞 accept);
不加重启探活兜底(用户裁决 2026-10-02:不治标);不做连接数限流(另一个问题,
无现场证据)。

## 实施记录(2026-10-02)

落点:`crates/app/src/serve.rs`(`TlsListener` 重写,`TLS_HANDSHAKE_TIMEOUT` 常量,
fixture `crates/app/tests/fixtures/tls-test-{cert,key}.pem`)。回归测试在旧代码上
会卡死在第 ⑤ 步——修复后 0.51s 跑绿。106 公网实测见票尾补记。
