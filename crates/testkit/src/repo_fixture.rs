//! GitHub 来源的离线 fixture（决策 194，票 01）：一个**裸仓** + 一个**离线 smart HTTP**。
//!
//! 分两层，各司其职（票 01 的「离线 fixture」一节）：
//!
//! - [`RepoFixture`]：临时目录里的裸仓，用来放技能目录。快单测用它覆盖扫描层、递归读子树、
//!   三类 not_found 与 `RepoId` 校验——但**覆盖不到传输策略**（local transport 连 shallow
//!   都不支持，libgit2 源码里那句 FIXME 至今还在）。
//! - [`SmartHttp`]：把同一个裸仓用 git 协议暴露成一个真 HTTP 端点，于是 `depth(1)`、
//!   `RemoteRedirect::None`、字节上限中断、按旧 commit 取物这些**只有走真传输才走得到**的
//!   分支才能在 `cargo test` 里稳定复现。
//!
//! 与 [`crate::git_fixture::Repo`] 同一体例：用**系统 git CLI** 搭场景。不引入 libgit2
//! 客户端代码是有意的——fixture 自己若也用被测的那套客户端，就等于拿被测物去搭自己的考场。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tempfile::TempDir;

/// 连接读写超时：fixture 出错时宁可 10 s 报错，也别让用例挂到 Playwright 的全局超时，
/// 那样连"是 fixture 先坏"都看不出来。
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// 请求体上限（解 `Content-Length` 时用）：libgit2 的 want/have 列表只有几百字节，
/// 真给出一个巨大的声明值说明对端不是我们要服务的那个客户端，直接拒掉比分配内存安全。
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// 临时的**裸仓** + 用于搭内容的工作仓。
///
/// 内容不能直接写进裸仓：裸仓没有工作树，`git add -A` 在那里必然失败。所以内部再开一个
/// 临时工作仓，在里面写文件、commit，再 `git push` 进裸仓——两个临时目录都活到
/// `RepoFixture` 被 drop 为止。
pub struct RepoFixture {
    /// 裸仓的父临时目录，**只承担生命周期**（前导下划线是有意的：除了活着之外没有别的用途，
    /// 而它一旦 drop，裸仓就没了）。
    _root: TempDir,
    /// `<root>/repo.git`：`SmartHttp::url()` 拼的是同一个名字，两边必须对齐。
    bare: PathBuf,
    /// 工作仓：写文件与 commit 都在这里发生。
    work: TempDir,
}

impl RepoFixture {
    /// 建裸仓（`init --bare -b main`）并**默认打开** `uploadpack.allowAnySHA1InWant`。
    pub fn new() -> Result<Self> {
        RepoFixture::named("repo")
    }

    /// 裸仓名可配：`SmartHttp` 只服务**这一个仓名**，别的仓名一律 404。
    ///
    /// 名字可配不是装饰：内置的推荐清单指向真实存在的仓（例如 `mattpocock/skills`），
    /// 而"一键安装走清单定位"这条链路要能离线测——那就得让 fixture 顶替**那个仓名**。
    /// 同时"只服务一个仓名"这条性质本身是被断言的对象：仓不存在与仓读不到要分得开
    /// （决策 194 的八类失败），fixture 若对任意仓名都应答，那条判定就测不出来。
    pub fn named(stem: &str) -> Result<Self> {
        let root = tempfile::Builder::new()
            .prefix("agentpipeline-repo-git-")
            .tempdir()
            .context("建裸仓临时目录失败")?;
        let bare = root.path().join(format!("{stem}.git"));
        run(
            root.path(),
            &["init", "--bare", "-b", "main", &bare.to_string_lossy()],
        )?;

        // 硬要求：不开这个位，libgit2 会在**客户端**就拦下「按裸 SHA 取对象」
        //（`fetch.c` 的 `cannot fetch a specific object from the remote repository`），
        // 现象是"还没发出任何 pack 请求就失败"——而真 GitHub 的广告里
        // `allow-tip-sha1-in-want` / `allow-reachable-sha1-in-want` 两个位都在。
        // 所以这里开它是让本地形态与上游一致，不是迁就 fixture。
        run(&bare, &["config", "uploadpack.allowAnySHA1InWant", "true"])?;

        let work = tempfile::Builder::new()
            .prefix("agentpipeline-repo-worktree-")
            .tempdir()
            .context("建工作仓临时目录失败")?;
        run(work.path(), &["init", "-b", "main"])?;

        Ok(RepoFixture {
            _root: root,
            bare,
            work,
        })
    }

    /// 在工作仓里写一个文件（相对路径，自动建父目录）。
    pub fn add_file(&mut self, rel: &str, contents: &str) -> Result<()> {
        let path = self.work.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("建目录失败：{}", parent.display()))?;
        }
        std::fs::write(&path, contents)
            .with_context(|| format!("写文件失败：{}", path.display()))?;
        Ok(())
    }

    /// 提交当前工作树的一份快照，push 进裸仓，返回新 commit 的 40 位 hex。
    ///
    /// 没有改动时 `git commit` 是非零退出，这里**如实报错**而不是静默返回旧 SHA——
    /// 静默的话调用方会以为自己造出了第二个 commit。
    pub fn commit(&mut self, message: &str) -> Result<String> {
        let bare = self.bare.to_string_lossy().to_string();
        run(self.work.path(), &["add", "-A"])?;
        run(self.work.path(), &["commit", "-m", message])?;
        run(self.work.path(), &["push", &bare, "main"])?;
        let sha = run(self.work.path(), &["rev-parse", "HEAD"])?;
        ensure_oid(sha.trim(), "工作仓 HEAD")
    }

    /// 裸仓里 `main` 的 tip（40 位 hex）。
    pub fn tip(&self) -> Result<String> {
        let sha = run(&self.bare, &["rev-parse", "main"])?;
        ensure_oid(sha.trim(), "裸仓 main")
    }

    /// 裸仓路径（系统 git 直接 `fetch` 它时需要补 `file://` 前缀，走 local transport）。
    pub fn dir(&self) -> &Path {
        &self.bare
    }
}

/// 远端的行为形态（票 23）：八类失败里有两类在正常服务的 fixture 上**打不到**，
/// 需要远端先摆出那个形态——都是真实存在过的服务端样子，不是任意错误注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteBehaviour {
    /// 照常服务（既有用例的默认形态）。
    Normal,
    /// 一切请求回 401：**私有仓、无凭据**的远端（`repo_unreadable` 的可测输入；
    /// 真 GitHub 对无凭据读私有仓回 401 还是 404 属实测缺口，见 `unreadable` 的注——这里钉的是
    /// 「401 形态分得出来」这条分类路径本身）。
    AuthRequired,
    /// 把 `git-upload-pack` 响应里的 pack **尾哈希**改坏（`digest_mismatch` 的可测输入）：
    /// 内容照流，只有「git 自己算出来的那个哈希」对不上——正是生产里
    /// `is_digest_shaped` 要接的那类失败（传输损坏 / 坏包）。
    CorruptPack,
}

/// 离线 **smart HTTP**：把裸仓用 git 协议暴露出来。
///
/// 只有两条路由，都由系统 git 的 `upload-pack` 子进程实现：
///
/// - `GET  /repo.git/info/refs?service=git-upload-pack` → `--advertise-refs`
/// - `POST /repo.git/git-upload-pack` → 请求体喂 stdin、stdout 当响应体
///
/// **为什么不是 dumb HTTP**：libgit2 硬校验响应的 `Content-Type`，dumb HTTP 那条路
/// （`git update-server-info` + 任意静态服务器）返回的是 `application/octet-stream`，
/// 实测被拒（`invalid content-type`）。故这里必须自己写那两行响应头、把 mock 做得像真服务器。
///
/// 服务随本结构 drop 而停：后台线程轮询 `stop` 标志（listener 设成非阻塞），
/// 不留给下一个用例。
pub struct SmartHttp {
    addr: SocketAddr,
    /// 裸仓名（不含 `.git`）。路由只认这一个仓名，别的仓名 404——见 `RepoFixture::named`。
    stem: String,
    /// 收到的请求行（含 query）。前端 E2E 靠"没点添加之前零请求"这断言钉住"没被诱导联网"，
    /// 所以这份日志必须准：请求行一解出来就入账，**早于**响应写出。
    requests: Arc<Mutex<Vec<String>>>,
    /// 每趟请求的**细读**（header、响应状态、收线方式），只为排查用。
    /// 与 [`SmartHttp::requests`] 分开：后者是断言口径（"零请求"那类），不能被诊断字段污染。
    trace: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl SmartHttp {
    /// 在本机回环上起服务（端口 0 = 让内核分配，避免并行用例撞端口）。默认 [`RemoteBehaviour::Normal`]。
    pub async fn serve(bare: &Path) -> Result<Self> {
        Self::serve_behaviour(bare, RemoteBehaviour::Normal).await
    }

    /// 同 [`serve`](Self::serve)，但远端按指定 [`RemoteBehaviour`] 应答（票 23 的两类失败形态）。
    pub async fn serve_behaviour(bare: &Path, behaviour: RemoteBehaviour) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).context("绑定回环端口失败")?;
        let addr = listener.local_addr().context("读回环端口失败")?;
        // 非阻塞轮询：这样后台线程能定期检查 stop，不必靠"关掉 listener 让 accept 报错"
        // 这种会泄漏 fd 的做法。
        listener
            .set_nonblocking(true)
            .context("设 listener 非阻塞失败")?;

        let bare = bare.to_path_buf();
        let stem = bare
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".git"))
            .context("裸仓目录名不是 `{stem}.git` 形态")?
            .to_string();
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let loop_trace = Arc::clone(&trace);
        let loop_requests = Arc::clone(&requests);
        let loop_stop = Arc::clone(&stop);
        let loop_stem = stem.clone();
        std::thread::spawn(move || {
            while !loop_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // 每个连接一个线程：libgit2 是 info/refs 与 upload-pack 两趟串行，
                        // 但共用一条连接池，串行处理会让第二趟等到超时。
                        let bare = bare.clone();
                        let requests = Arc::clone(&loop_requests);
                        let trace = Arc::clone(&loop_trace);
                        let stem = loop_stem.clone();
                        std::thread::spawn(move || {
                            if let Err(err) = handle_connection(
                                stream, &bare, &stem, behaviour, &requests, &trace,
                            ) {
                                // fixture 的失败要看得见，而不是变成一个卡住的 fetch
                                let detail = last_trace(&trace);
                                eprintln!("[SmartHttp] 处理连接失败：{err:#}（上一趟：{detail}）");
                            }
                        });
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => {
                        eprintln!("[SmartHttp] accept 失败，服务退出：{err}");
                        break;
                    }
                }
            }
        });

        Ok(SmartHttp {
            addr,
            stem,
            requests,
            trace,
            stop,
        })
    }

    /// `http://127.0.0.1:<port>/{裸仓名}.git`（单段形态，与 [`RepoFixture`] 的裸仓名对齐）。
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}.git", self.addr.port(), self.stem)
    }

    /// 裸仓名（不含 `.git`）。
    pub fn stem(&self) -> &str {
        &self.stem
    }

    /// 只到主机端口的基地址——正是生产里 `AGENTPIPELINE_MARKET_GIT_BASE` 的形态
    /// （URL 由 `{base}/{owner}/{repo}.git` 拼）。
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    /// 生产形态的地址：`{base}/{owner}/{repo}.git`。
    ///
    /// 生产里 URL 是 `{AGENTPIPELINE_MARKET_GIT_BASE}/{owner}/{repo}.git`，而 `RepoId`
    /// 要求恰好一个 `/`（两段），所以拿本 fixture 当后端时得用这个形态：
    /// `AGENTPIPELINE_MARKET_GIT_BASE=<base>` + 仓名 `{owner}/repo`。
    /// [`url`](Self::url) 那个单段形态留着，是 fixture 自己那几条自检用例在用的。
    pub fn url_for(&self, owner: &str) -> String {
        format!(
            "http://127.0.0.1:{}/{owner}/{}.git",
            self.addr.port(),
            self.stem
        )
    }

    /// 迄今为止收到的请求行（克隆一份，调用方拿去断言）。
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("请求日志锁中毒").clone()
    }

    /// 每趟请求的细读：请求行 + 我方解出来的 `Content-Length` / 分块标志 / 对端的
    /// `Connection`、以及响应状态与收线方式。断言失败时把它一起报出来——这条路上的红
    /// 多半是"客户端说 broken pipe、服务端觉得自己什么都答了"，没有这份就只能猜。
    pub fn trace(&self) -> Vec<String> {
        self.trace.lock().expect("诊断日志锁中毒").clone()
    }
}

impl Drop for SmartHttp {
    fn drop(&mut self) {
        // 只置标志：后台线程在 5 ms 内看到并退出，listener 随之 drop。
        // 这里**不 join**——drop 可能发生在异步上下文里，join 会把线程调度问题
        // 变成用例挂起。
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 连接级处理：一趟请求一条连接。
///
/// **为什么不是 keep-alive**：实测 libgit2 1.9.7 在这里靠服务端关连接来标记响应结束——
/// 改成 keep-alive（响应写 `Connection: keep-alive` 且不关）之后 13/21 个用例当场红成
/// `unexpected EOF`，单跑也必红。故响应照旧声明 `Connection: close` 并关闭。
fn handle_connection(
    stream: TcpStream,
    bare: &Path,
    stem: &str,
    behaviour: RemoteBehaviour,
    requests: &Mutex<Vec<String>>,
    trace: &Mutex<Vec<String>>,
) -> Result<()> {
    // **必须显式设回阻塞**（实测，macOS）：`accept()` 返回的 socket **继承监听 socket 的
    // O_NONBLOCK**，于是它一出生就是非阻塞的——`read_line` 在客户端数据还没到时立刻
    // `EAGAIN` 返回，我们当场把连接收掉，客户端那条请求撞上 FIN/RST，报的是
    // `unexpected EOF` / `Broken pipe` / `Connection reset by peer`。它表现为**偶发**红：
    // 客户端的请求先到就先赢，accept 快一步就输（本仓实测并发跑约 1/8 的用例轮次会中，
    // 单跑必过）。设成阻塞之后读会老老实实等 `SO_RCVTIMEO`，下面那两个超时才真的是超时。
    stream.set_nonblocking(false).context("设连接为阻塞失败")?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .context("设读超时失败")?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .context("设写超时失败")?;
    let mut reader = BufReader::new(stream.try_clone().context("克隆连接失败")?);
    let mut writer = stream;
    serve_one(
        &mut reader,
        &mut writer,
        bare,
        stem,
        behaviour,
        requests,
        trace,
    )
}

/// 记一笔诊断（带锁，出错也不影响服务）。
fn note_trace(trace: &Mutex<Vec<String>>, line: impl Into<String>) {
    if let Ok(mut all) = trace.lock() {
        all.push(line.into());
    }
}

/// 诊断日志的最后一行（fixture 出错时报出来）。
fn last_trace(trace: &Mutex<Vec<String>>) -> String {
    trace
        .lock()
        .ok()
        .and_then(|all| all.last().cloned())
        .unwrap_or_else(|| "（无）".into())
}

/// 这类读错误说明**对端不打算再说话了**，不是 fixture 自己的故障。
///
/// 读超时是常态：客户端（libgit2）把连接留在池子里又不用，或握完手就没再发请求。
/// 静默收掉即可——报出去只会让并发用例的日志里混进一行吓人的东西，而真出事时失败
/// 在客户端那一侧（broken pipe）看得见。
fn is_peer_gone(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

/// 读一趟请求、答一趟响应。
fn serve_one(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
    bare: &Path,
    stem: &str,
    behaviour: RemoteBehaviour,
    requests: &Mutex<Vec<String>>,
    trace: &Mutex<Vec<String>>,
) -> Result<()> {
    // —— 请求行 ——
    // 这三条**静默收线**的路都要留一笔诊断：它们意味着"客户端连上来了却没拿到响应"，
    // 而客户端那一侧只会报 broken pipe / unexpected EOF——不留痕就只能两边干猜。
    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => return Ok(()), // 对端正常收线
        Ok(_) => {}
        Err(err) if is_peer_gone(&err) => {
            note_trace(trace, format!("连接在读请求行前闲置/收线：{err}"));
            return Ok(());
        }
        Err(err) => return Err(err).context("读请求行失败"),
    }
    let mut parts = request_line.trim_end().split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() {
        respond(
            writer,
            "400 Bad Request",
            "text/plain",
            format!("坏请求行：{request_line:?}").as_bytes(),
        )?;
        return Ok(());
    }
    // 入账要早于响应：E2E 的"零请求"断言不该受响应写出时机影响
    requests
        .lock()
        .expect("请求日志锁中毒")
        .push(format!("{method} {target}"));

    // —— headers ——
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut peer_wants_close = false;
    let mut raw_headers: Vec<String> = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).context("读 header 失败")? == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = value.trim().parse().ok(),
                "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => {
                    chunked = true
                }
                "connection" if value.to_ascii_lowercase().contains("close") => {
                    peer_wants_close = true
                }
                _ => {}
            }
        }
        raw_headers.push(line);
    }

    // —— body ——
    // libgit2 的请求体一般带 Content-Length，但别假设；分块解不了就 501 并把原始 header
    // 记进请求日志，好排查是谁在用别的客户端打这个 fixture。
    let body = if chunked {
        match read_chunked(reader) {
            Ok(body) => body,
            Err(err) => {
                requests.lock().expect("请求日志锁中毒").push(format!(
                    "501 无法解分块请求体：{err:#}；原始 headers：{raw_headers:?}"
                ));
                respond(
                    writer,
                    "501 Not Implemented",
                    "text/plain",
                    b"chunked request body not supported",
                )?;
                note_trace(trace, format!("{method} {target} → 501 分块请求体解不了"));
                return Ok(());
            }
        }
    } else if let Some(len) = content_length {
        if len > MAX_REQUEST_BYTES {
            bail!("请求体声明 {len} 字节，超过 fixture 上限 {MAX_REQUEST_BYTES}");
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).context("读请求体失败")?;
        buf
    } else {
        Vec::new()
    };

    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target.as_str(), ""),
    };

    // —— 形态②：私有仓、无凭据（票 23）——
    // 请求行 / header / body 都**读完**再答：没读完就收线，`close()` 时队列里压着的字节会
    // 换来 RST，把已写出的响应一起丢掉（同 `handle_connection` 里那条 macOS 实测）。
    if behaviour == RemoteBehaviour::AuthRequired {
        respond(
            writer,
            "401 Unauthorized",
            "text/plain",
            b"Authentication required",
        )?;
        note_trace(
            trace,
            format!("{method} {target} → 401（AuthRequired 形态）"),
        );
        return Ok(());
    }

    // —— 两条路由 ——
    let note = format!(
        "{} {} len={content_length:?} chunked={chunked} 对端要关={peer_wants_close} 原始 headers={raw_headers:?}",
        method, target
    );
    if method == "GET"
        && strip_repo_prefix(path, stem) == Some("info/refs")
        && query.contains("service=git-upload-pack")
    {
        // 不设 GIT_PROTOCOL 环境变量 ⇒ upload-pack 说协议 v0。这是刻意的：本仓的 libgit2
        // 1.9.7 只会 v0，且 `allow-*-sha1-in-want` 两个能力位只在 v0 的广告里出现。
        let advertised = match run_upload_pack(bare, true, &[]) {
            Ok(out) => out,
            Err(err) => return respond_500(writer, err),
        };
        // 广告体前面这两段是 smart HTTP 的规定动作，少了 git 客户端会当成 dumb HTTP 直接放弃。
        // **服务行本身也是一个 pkt-line**（长度前缀 + `# service=...\n`，26 + 4 = 0x1e），
        // 少了那 4 个字节的十六进制长度，git 客户端会报
        // `protocol error: bad line length character: # se`。
        let service_line = b"# service=git-upload-pack\n";
        let mut body = Vec::new();
        body.extend_from_slice(format!("{:04x}", service_line.len() + 4).as_bytes());
        body.extend_from_slice(service_line);
        body.extend_from_slice(b"0000");
        body.extend_from_slice(&advertised);
        respond(
            writer,
            "200 OK",
            "application/x-git-upload-pack-advertisement",
            &body,
        )?;
    } else if method == "POST" && strip_repo_prefix(path, stem) == Some("git-upload-pack") {
        match run_upload_pack(bare, false, &body) {
            Ok(mut out) => {
                // 形态③：坏包（票 23）——定位不到 pack 是 fixture 自己坏了，走 eprintln 报出来，
                // 别静默送出一份好包让用例假绿。
                if behaviour == RemoteBehaviour::CorruptPack {
                    corrupt_pack_tail(&mut out)?;
                }
                respond(
                    writer,
                    "200 OK",
                    "application/x-git-upload-pack-result",
                    &out,
                )?
            }
            Err(err) => return respond_500(writer, err),
        }
    } else {
        respond(writer, "404 Not Found", "text/plain", b"not found")?;
    }
    // 只有响应真的写出去了才记这一笔：排查时要看的是「服务端答了什么」
    let after = if let Ok(extra) = probe_pending(reader) {
        format!("，响应后另有 {extra} 字节在途")
    } else {
        String::new()
    };
    let line = format!("{note} → 已响应{after}");
    if std::env::var_os("AGENTPIPELINE_FIXTURE_TRACE").is_some() {
        let port = writer.local_addr().map(|a| a.port()).unwrap_or(0);
        eprintln!("[SmartHttp:{port}] {line}");
    }
    note_trace(trace, line);
    Ok(())
}

/// 响应写出后**对端还有多少字节已经在途**（一次非阻塞读，不等待、不占时间）。
///
/// 这一个数字是排查"客户端说 broken pipe / unexpected EOF、而服务端觉得自己答完了"的钥匙：
/// `close()` 时接收队列里还压着没读的字节，内核回的是 RST——**RST 会把已发出的响应一起丢掉**，
/// 客户端于是看到 `unexpected EOF`（响应短了）或 `Connection reset by peer`。
///
/// 只探一次、不设超时：这条纯属诊断，默认路径上不该为它多花一毫秒（真凶已经定了，
/// 见 [`handle_connection`] 里那个"macOS 的 accepted socket 继承 O_NONBLOCK"）。
fn probe_pending(reader: &mut BufReader<TcpStream>) -> Result<usize> {
    reader
        .get_ref()
        .set_nonblocking(true)
        .context("设连接为非阻塞失败")?;
    let mut total = reader.buffer().len();
    let mut scratch = [0u8; 8192];
    loop {
        match reader.get_mut().read(&mut scratch) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break, // WouldBlock（没有更多）/ 收线
        }
    }
    Ok(total)
}

/// 把 `/{stem}.git/...` 或 `/{owner}/{stem}.git/...` 里的仓名部分剥掉，返回剩下的段。
///
/// 两种形态都要认：前者是 fixture 自己的速记（[`SmartHttp::url`]），后者是**生产里的
/// 真实形态**（`{base}/{owner}/{repo}.git`）——拿本 fixture 当后端时用的是后者，
/// 因为 `RepoId` 要求恰好一个 `/`（两段）。
///
/// **仓名必须精确等于 `stem`**：这条路只服务一个仓，别的仓名要 404——那正是"仓不存在"
/// 这一类失败的来源（决策 194 的八类要分得开，fixture 若对任意仓名都应答，
/// 那条判定就没有可测的输入）。多段前缀（`/a/b/x.git`）同样不算。
fn strip_repo_prefix<'a>(path: &'a str, stem: &str) -> Option<&'a str> {
    let rest = path.strip_prefix('/')?;
    let marker = format!("{stem}.git/");
    // 形态一：`/{stem}.git/<tail>`（`SmartHttp::url` 那个速记）
    if let Some(tail) = rest.strip_prefix(marker.as_str()) {
        return Some(tail);
    }
    // 形态二：`/{owner}/{stem}.git/<tail>`（生产形态 `{base}/{owner}/{repo}.git`）
    let (owner, tail) = rest.split_once('/')?;
    if owner.is_empty() {
        return None;
    }
    tail.strip_prefix(marker.as_str())
}

/// 跑一次 `git upload-pack`。
///
/// stdin / stdout **必须并发读写**：把请求体全写完再读响应，在请求体大于管道缓冲、
/// 或响应体大到让 upload-pack 阻塞在写上的时候，两边会互等成死锁。
///
/// **非零退出码不等于这次服务失败**：upload-pack 对协议级的拒绝（最典型的是
/// `want` 一个它没有的对象）会往 stdout 写一条 `ERR upload-pack: not our ref …` 的
/// pkt-line **然后非零退出**。真的 `git http-backend` 是不看退出码的，它把 stdout 原样
/// 流给客户端、HTTP 状态仍是 200——传输本身成功了，失败是 git 协议自己报的。
/// fixture 若在这里改成 500，客户端看到的是"服务端坏了"（`unexpected http status code: 500`），
/// 于是"那个 commit 取不到"会表现成"网络坏了"，正好毁掉决策 194 八类失败要分得开这件事。
/// 故只有**一个字都没写出来**时才当 fixture 自己坏了（裸仓被删之类）。
fn run_upload_pack(bare: &Path, advertise_refs: bool, input: &[u8]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("upload-pack");
    if advertise_refs {
        cmd.arg("--advertise-refs");
    }
    cmd.arg("--stateless-rpc").arg(bare);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("启动 git upload-pack 失败")?;

    let mut stdin = child.stdin.take().context("取 upload-pack stdin 失败")?;
    let payload = input.to_vec();
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
        // 这里 drop stdin：stateless-rpc 靠 EOF 判定"请求结束"
    });

    let mut stderr = child.stderr.take().context("取 upload-pack stderr 失败")?;
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .context("取 upload-pack stdout 失败")?
        .read_to_end(&mut stdout)
        .context("读 upload-pack 输出失败")?;
    let status = child.wait().context("等待 upload-pack 退出失败")?;
    let _ = feeder.join();
    let stderr = err_reader.join().unwrap_or_default();
    if !status.success() && stdout.is_empty() {
        bail!("git upload-pack 退出码 {:?}：{stderr}", status.code());
    }
    Ok(stdout)
}

/// 把 `git upload-pack` 响应里的 pack **尾哈希**改坏一个字节（[`RemoteBehaviour::CorruptPack`]）。
///
/// 只动内容、不动 framing：pkt-line 长度头一个都不改，libgit2 收包照常，坏的恰是「git 自己
/// 算出来的那个 sha1」——这正是 `digest_mismatch` 的语义（传输损坏 / 坏包），而不是网络失败。
///
/// v0 `--stateless-rpc` 的形态：先是协商的 pkt-line（`NAK` / `ACK`），pack 要么**裸**跟在
/// 后面（未开 side-band），要么裹在 `0x01` 信道的数据帧里（开了 side-band）。两种形态下
/// 「尾部 20 字节」都是 pack 的 sha1；本 fixture 的仓都很小、pack 一帧装得下，故数据帧的
/// 帧尾就是 pack 尾。定位不到 `PACK` 魔数或 pkt-line 对不上就**报错**——fixture 自己坏了
/// 要看得见（走 eprintln），不许静默送出一份好包让用例假绿。
fn corrupt_pack_tail(out: &mut [u8]) -> Result<()> {
    let mut pos = 0usize;
    while pos < out.len() {
        // 形态①：裸 pack（未开 side-band）——协商 pkt-line 之后直接就是它
        if out[pos..].starts_with(b"PACK") {
            out[out.len() - 1] ^= 0xFF;
            return Ok(());
        }
        // 形态②：pkt-line（flush / 协商行 / side-band 帧），四字节长度头必须是十六进制
        let head = out
            .get(pos..pos + 4)
            .context("响应在 pkt-line 长度头之前就结束了")?;
        if !head.iter().all(|b| b.is_ascii_hexdigit()) {
            bail!("{pos} 处既不是 `PACK` 也不是 pkt-line 头：形态与预期不符");
        }
        let len = usize::from_str_radix(std::str::from_utf8(head)?, 16)
            .context("pkt-line 长度头不是十六进制")?;
        if len == 0 {
            // flush（`0000`）：长度 0 是特殊值——占 4 字节、无 payload（实测协商段与 pack
            // 之间真有一条，把它当畸形会让连接当场断掉、客户端只看到 unexpected EOF）
            pos += 4;
            continue;
        }
        if len < 4 {
            bail!("pkt-line 长度 {len} < 4（畸形）");
        }
        let end = pos + len;
        let payload = out.get(pos + 4..end).context("pkt-line 声明的长度越界")?;
        if payload.first() == Some(&0x01) && payload.get(1..5) == Some(&b"PACK"[..]) {
            // 数据帧 = `0x01` 信道字节 + pack；小仓一帧装下 ⇒ 帧尾即 pack 的尾哈希
            out[end - 1] ^= 0xFF;
            return Ok(());
        }
        pos = end; // flush（`0000` → 空 payload）与协商行都直接跳过
    }
    // 这一轮没有 pack 是**正常形态**：`depth(1)` 的浅取协商第一轮就是
    // `shallow <sha>` + flush（实测 56 字节，pack 在下一轮 POST 里，那一段照样会走
    // 本函数）。走完没找到、而 `PACK` 魔数确实在某处——那才是 pkt 走法的形态回归，
    // 报出来（静默送出好包会让 digest 用例假绿，这条 bail 就是反假绿的哨兵）。
    if out.windows(4).position(|w| w == b"PACK").is_none() {
        return Ok(());
    }
    bail!(
        "这一轮有 pack（偏移 {:?}）但 pkt 走法没走到它——形态回归（len={}，头 96 字节：{:02x?}）",
        out.windows(4).position(|w| w == b"PACK"),
        out.len(),
        &out[..out.len().min(96)]
    )
}

/// 解 `Transfer-Encoding: chunked`（够用的一层：hex 长度行 + CRLF + 尾 trailer）。
fn read_chunked<R: BufRead>(reader: &mut R) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line).context("读分块长度失败")? == 0 {
            bail!("分块请求体在长度行之前就结束了");
        }
        let raw = size_line.trim();
        if raw.is_empty() {
            continue;
        }
        // `1a;ext=1` 这种带扩展的写法按规范要把扩展段切掉
        let size_text = raw.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .with_context(|| format!("分块长度不是十六进制：{size_text:?}"))?;
        if size == 0 {
            // 末块之后是 trailer，读到空行为止
            loop {
                let mut trailer = String::new();
                if reader.read_line(&mut trailer).context("读 trailer 失败")? == 0 {
                    break;
                }
                if trailer.trim().is_empty() {
                    break;
                }
            }
            break;
        }
        if out.len() + size > MAX_REQUEST_BYTES {
            bail!("分块请求体超过 fixture 上限 {MAX_REQUEST_BYTES}");
        }
        let mut buf = vec![0u8; size];
        reader.read_exact(&mut buf).context("读分块数据失败")?;
        out.extend_from_slice(&buf);
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).context("读分块尾 CRLF 失败")?;
    }
    Ok(out)
}

/// 写一个完整响应并关闭写半边（`Connection: close`）。
///
/// 收发两侧的收线分工要留着：`shutdown(Write)` 发 FIN，而**读半边不动**——这样客户端
/// 迟到的字节还有地方落，不会在 `close()` 那一刻攒成一个 RST。
fn respond(writer: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    writer.write_all(head.as_bytes()).context("写响应头失败")?;
    writer.write_all(body).context("写响应体失败")?;
    writer.flush().context("flush 响应失败")?;
    let _ = writer.shutdown(Shutdown::Write);
    Ok(())
}

/// upload-pack 自己失败（例如裸仓被删）时给一个能看懂的 500。
fn respond_500(writer: &mut TcpStream, err: anyhow::Error) -> Result<()> {
    let text = format!("upload-pack 失败：{err:#}");
    respond(
        writer,
        "500 Internal Server Error",
        "text/plain",
        text.as_bytes(),
    )
}

/// 校验 40 位十六进制：缩写 SHA 在 fetch 里会**返回 Ok 但什么都没取**（实测），
/// 静默空转是这条路上最贵的失败形态，故在 fixture 出口就拦住。
fn ensure_oid(sha: &str, what: &str) -> Result<String> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{what} 不是 40 位十六进制 SHA：{sha:?}");
    }
    Ok(sha.to_string())
}

/// 执行 git 并要求成功，返回 stdout。
fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let (code, stdout, stderr) = run_raw(cwd, args);
    if code != 0 {
        bail!("git {} 失败（退出码 {code}）：{stderr}", args.join(" "),);
    }
    Ok(stdout)
}

/// 执行 git 不校验退出码：(exit_code, stdout, stderr)。
fn run_raw(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new("git")
        .args(["-c", "user.name=fixture"])
        .args(["-c", "user.email=fixture@localhost"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git 可执行");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个能当 fetch 客户端用的空仓（不提交任何东西，只借它的 `.git`）。
    fn client_repo() -> TempDir {
        let dir = tempfile::Builder::new()
            .prefix("agentpipeline-fetch-client-")
            .tempdir()
            .unwrap();
        run(dir.path(), &["init", "-b", "main"]).unwrap();
        dir
    }

    #[test]
    fn new_builds_a_bare_repo_and_commits_land_in_it() {
        let mut repo = RepoFixture::new().unwrap();
        assert!(repo.dir().join("HEAD").exists(), "裸仓应有 HEAD");

        repo.add_file("skills/demo/SKILL.md", "---\nname: demo\n---\n\n正文\n")
            .unwrap();
        let first = repo.commit("feat: 第一个技能").unwrap();
        assert_eq!(first.len(), 40);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(repo.tip().unwrap(), first);

        repo.add_file("skills/other/SKILL.md", "---\nname: other\n---\n\n正文\n")
            .unwrap();
        let second = repo.commit("feat: 第二个技能").unwrap();
        assert_eq!(repo.tip().unwrap(), second);
        assert_ne!(first, second, "两次 commit 的 tip 必须不同");
    }

    #[test]
    fn bare_repo_enables_allow_any_sha1_in_want() {
        let repo = RepoFixture::new().unwrap();
        let (code, stdout, stderr) = run_raw(
            &repo.bare,
            &["config", "--get", "uploadpack.allowAnySHA1InWant"],
        );
        assert_eq!(code, 0, "读不到该配置：{stderr}");
        assert_eq!(stdout.trim(), "true");
    }

    #[tokio::test]
    async fn smart_http_advertises_main_to_the_system_git_client() {
        let mut repo = RepoFixture::new().unwrap();
        repo.add_file("skills/demo/SKILL.md", "---\nname: demo\n---\n\n正文\n")
            .unwrap();
        let tip = repo.commit("feat: 第一个技能").unwrap();

        let http = SmartHttp::serve(repo.dir()).await.unwrap();
        let url = http.url();

        // 用系统 git 当客户端，而不是 libgit2：fixture 先被独立验证一遍，
        // 免得到时候分不清是 fixture 坏了还是被测的客户端代码坏了。
        // `-c protocol.version=0` 与本机默认（v2）不同——但 v0 正是 libgit2 会说的话。
        let out = Command::new("git")
            .args(["-c", "protocol.version=0", "ls-remote", &url])
            .output()
            .expect("git 可执行");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(out.status.code(), Some(0), "ls-remote 失败：{stderr}");
        assert!(
            stdout.contains(&format!("{tip}\trefs/heads/main")),
            "广告里应有 main 的 tip {tip}：{stdout}"
        );

        // 请求日志：http-backend 的形态就是先 GET info/refs?service=git-upload-pack
        let requests = http.requests();
        assert!(
            requests
                .iter()
                .any(|line| line == "GET /repo.git/info/refs?service=git-upload-pack"),
            "请求日志应有 info/refs 那一行：{requests:?}"
        );
    }

    #[tokio::test]
    async fn smart_http_serves_an_old_commit_by_bare_sha() {
        let mut repo = RepoFixture::new().unwrap();
        repo.add_file("skills/demo/SKILL.md", "---\nname: demo\n---\n\n第一版\n")
            .unwrap();
        let old = repo.commit("feat: 第一版").unwrap();
        repo.add_file("skills/demo/SKILL.md", "---\nname: demo\n---\n\n第二版\n")
            .unwrap();
        let tip = repo.commit("feat: 第二版").unwrap();
        assert_ne!(old, tip);

        let http = SmartHttp::serve(repo.dir()).await.unwrap();
        let url = http.url();
        let client = client_repo();
        let client_path = client.path().to_string_lossy().to_string();

        // 按**裸 SHA** 取一个非 tip 的旧 commit。这条命令成不成功，取决于裸仓的
        // `uploadpack.allowAnySHA1InWant` 开没开——正是 `RepoFixture::new()` 要钉住的那一点。
        // `protocol.version=0` 是因为本机 git 默认走 v2，而 v2 的广告里没有
        // `allow-*-sha1-in-want` 那两个能力位（libgit2 1.9.7 只会 v0）。
        let (code, stdout, stderr) = run_raw(
            Path::new(&client_path),
            &["-c", "protocol.version=0", "fetch", &url, &old],
        );
        assert_eq!(code, 0, "按旧 SHA fetch 失败：{stderr}");
        assert!(
            stdout.contains(&old) || stderr.contains(&old),
            "fetch 输出里应提到 {old}：{stdout} / {stderr}"
        );
        // `Ok` 不等于"取到了"（实测：缩写 SHA 与不存在的 refspec 都会返回 Ok 而什么都不取），
        // 所以要复验对象真的落了地。
        let (code, out, stderr) = run_raw(Path::new(&client_path), &["rev-parse", "FETCH_HEAD"]);
        assert_eq!(code, 0, "读 FETCH_HEAD 失败：{stderr}");
        assert_eq!(out.trim(), old, "取到的应是旧 commit，而不是 tip");
    }

    /// **回归**：连上来之后**先沉默一会儿再发请求**，服务端也必须伺候。
    ///
    /// 这一条钉的是那个 `EAGAIN` 竞态（本仓实测并发跑约 1/8 的用例轮次会中）：macOS 的
    /// `accept()` 会让新 socket **继承监听 socket 的 O_NONBLOCK**，于是它一出生就是非阻塞的，
    /// `read_line` 在客户端数据还没到时立刻返回 `EAGAIN`，我们当场把连接收掉——客户端那条
    /// 请求撞上 FIN/RST，报 `unexpected EOF` / `Broken pipe`，而 fixture 这边**一个请求行都没记上**。
    /// 症状是"偶发、单跑必过"，最难查的那类。
    ///
    /// 睡眠是刻意的：它**制造**那个时序（accept 先到、请求后到）。真修好了，
    /// 服务端会老老实实等到请求；没修好，这条会读到 EOF 或连接被关。
    #[tokio::test]
    async fn a_client_that_speaks_late_is_still_served() {
        let mut repo = RepoFixture::new().unwrap();
        repo.add_file("skills/demo/SKILL.md", "---\nname: demo\n---\n\n正文\n")
            .unwrap();
        repo.commit("feat: 第一个技能").unwrap();

        let http = SmartHttp::serve(repo.dir()).await.unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", http.addr.port())).expect("连 fixture");
        // 连上就晾着：服务端那边 accept 早已返回、read_line 已经进去了
        std::thread::sleep(Duration::from_millis(200));
        stream
            .write_all(
                b"GET /repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: fixture\r\n\r\n",
            )
            .expect("发请求");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("读响应");
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "静默 200ms 之后仍然要拿到广告：{response:.120}"
        );
        assert!(
            response.contains("application/x-git-upload-pack-advertisement"),
            "内容类型要是 smart HTTP 的广告：{response:.200}"
        );
        assert_eq!(
            http.requests(),
            vec!["GET /repo.git/info/refs?service=git-upload-pack".to_string()],
            "这一趟请求必须被记上——记不上正是那个竞态的症状"
        );
    }

    #[tokio::test]
    async fn unknown_path_is_404_and_gets_logged() {
        let repo = RepoFixture::new().unwrap();
        let http = SmartHttp::serve(repo.dir()).await.unwrap();

        // 直连一个不存在的路径，确认服务是活的、且 404 也会入账
        let mut stream = TcpStream::connect(("127.0.0.1", http.addr.port())).unwrap();
        write!(stream, "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 404"), "响应：{response}");
        assert!(
            http.requests().iter().any(|line| line == "GET /nope"),
            "404 的请求行也应入账：{:?}",
            http.requests()
        );
    }

    #[tokio::test]
    async fn chunked_body_from_an_unknown_client_is_501() {
        let repo = RepoFixture::new().unwrap();
        let http = SmartHttp::serve(repo.dir()).await.unwrap();

        // 这一段是替 libgit2 之外的手（或将来某个版本的客户端）探路：分块体若解不了，
        // 绝不能静默当成空 body 继续跑——那会变成一句谁也看不懂的协议错。
        let mut stream = TcpStream::connect(("127.0.0.1", http.addr.port())).unwrap();
        write!(
            stream,
            "POST /repo.git/git-upload-pack HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nnot-hex\r\n0\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 501"), "响应：{response}");
        // 诊断要能落到请求日志里：谁发的、原始 header 长什么样
        assert!(
            http.requests()
                .iter()
                .any(|line| line.starts_with("501 无法解分块请求体")),
            "501 的原始 header 应记进请求日志：{:?}",
            http.requests()
        );
    }
}
