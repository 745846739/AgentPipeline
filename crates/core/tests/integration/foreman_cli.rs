//! L2 集成：对讲台 CLI 引擎回合（决策 423 / 424，票 talk-cli-engine 02）。
//!
//! 测的是**外部可观测的行为**：CLI 会话的整段回路——回话落台账、CLI session id
//! 回填、第二轮 `--resume` 续接、resume 失败的显式重建（决策 424③）。真 claude
//! 不可依赖（CI 没装），全部经 `with_cli_program` 指到假脚本；脚本把收到的参数
//! 折成单行逐次落日志，续接与否、重建与否都咬在**日志证据**上。
//!
//! FakeAgent 的替换边界照旧（决策 148）的 CLI 版：LLM 替身在这条路上完全不被调用
//! （CLI 是整段执行器），工具与存储全部真跑。

use std::path::PathBuf;
use std::sync::Arc;

use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest};
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::ForemanRunner;
use agentpipeline_core::storage::foreman::{FOREMAN_ENGINE_CLI, FOREMAN_SESSION_KIND_TALK};
use agentpipeline_core::storage::Store;
use futures::future::BoxFuture;
use testkit::{ManualClock, TestHome};

/// CLI 路径完全不碰 LLM：给一个「被调到即报错」的空实现，替身边界自己钉自己。
struct NeverLlm;

impl LlmClient for NeverLlm {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        Box::pin(async { Err(agentpipeline_core::Error::Llm("CLI 路径不该碰 LLM".into())) })
    }
}

struct Fixture {
    _home: TestHome,
    store: Store,
    runner: ForemanRunner,
    /// 假 claude 的参数日志：每次调用一行（参数折行拍平）。
    fake_log: PathBuf,
}

/// 假 claude：把 argv 拍平记进日志（消息里的换行不能把日志行弄碎，断言按次计数）。
/// `resume_fails` = 带 `--resume` 的调用一律退出码 1（模拟 CLI 侧会话丢失）。
fn script_body(log: &str, resume_fails: bool) -> String {
    let guard = if resume_fails { "true" } else { "false" };
    format!(
        r#"#!/bin/sh
FAKE_LOG='{log}'
printf '%s\n' "$(echo "$*" | tr '\n' ' ')" >> "$FAKE_LOG"
if {guard} && printf '%s' "$*" | grep -q -- '--resume'; then
  echo "No conversation found with session ID sess-1" >&2
  exit 1
fi
echo '{{"type":"system","subtype":"init","session_id":"sess-1"}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"部分文本"}}]}}}}'
if printf '%s' "$*" | grep -q -- '--resume'; then
  echo '{{"type":"result","subtype":"success","is_error":false,"result":"续接成功。","usage":{{"input_tokens":100,"output_tokens":10}}}}'
else
  echo '{{"type":"result","subtype":"success","is_error":false,"result":"你好，值班经理。","usage":{{"input_tokens":50,"output_tokens":8}}}}'
fi
"#
    )
}

fn write_script(path: &PathBuf, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Fixture {
    async fn new(resume_fails: bool) -> Self {
        let home = TestHome::new().unwrap();
        let store = Store::open(home.home().clone(), Arc::new(ManualClock::fixed()))
            .await
            .unwrap();
        let bin = home.scratch_dir("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("fake-claude");
        let fake_log = bin.join("calls.log");
        write_script(
            &script,
            &script_body(&fake_log.display().to_string(), resume_fails),
        );
        let runner = ForemanRunner::new(
            store.clone(),
            Settings::default(),
            home.home().clone(),
            Arc::new(NeverLlm) as Arc<dyn LlmClient>,
            Arc::new(testkit::SseRecorder::new()),
        )
        .with_cli_program(script);
        Fixture {
            _home: home,
            store,
            runner,
            fake_log,
        }
    }

    async fn cli_session(&self) -> String {
        self.store
            .create_foreman_session_with_engine(FOREMAN_SESSION_KIND_TALK, "", FOREMAN_ENGINE_CLI)
            .await
            .unwrap()
            .id
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.fake_log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

#[tokio::test]
async fn cli_turn_persists_reply_tokens_and_session_id() {
    let f = Fixture::new(false).await;
    let session_id = f.cli_session().await;
    let turn = f
        .runner
        .say(Some(&session_id), "在吗")
        .await
        .expect("CLI 回合应成功");
    assert_eq!(turn.reply, "你好，值班经理。");
    assert_eq!((turn.prompt_tokens, turn.completion_tokens), (50, 8));

    let session = f
        .store
        .get_foreman_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session.cli_session_id.as_deref(),
        Some("sess-1"),
        "CLI session id 回填（决策 424③）"
    );

    let messages = f
        .store
        .list_foreman_messages(&session_id, 10, None)
        .await
        .unwrap();
    let assistant: Vec<_> = messages.iter().filter(|m| m.role == "assistant").collect();
    assert_eq!(assistant.len(), 1);
    assert!(
        assistant[0].briefing_json.is_some(),
        "态势快照随行落库（审计口径）"
    );
}

#[tokio::test]
async fn second_turn_resumes_cli_session() {
    let f = Fixture::new(false).await;
    let session_id = f.cli_session().await;
    f.runner.say(Some(&session_id), "在吗").await.unwrap();
    let turn = f.runner.say(Some(&session_id), "接着说").await.unwrap();
    assert_eq!(turn.reply, "续接成功。");

    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        !calls[0].contains("--resume"),
        "首轮不带 --resume：{calls:?}"
    );
    assert!(
        calls[1].contains("--resume sess-1"),
        "第二轮用回填的 id 续接（决策 424③）：{calls:?}"
    );
}

#[tokio::test]
async fn resume_failure_rebuilds_with_digest_and_notice() {
    let f = Fixture::new(true).await;
    let session_id = f.cli_session().await;
    // 首轮（无 resume）成功，领到 sess-1。
    f.runner.say(Some(&session_id), "在吗").await.unwrap();
    // 第二轮 resume 失败 → 重建：不静默（system 账）、不带 --resume 重跑、回话照常。
    let turn = f.runner.say(Some(&session_id), "接着说").await.unwrap();
    assert_eq!(turn.reply, "你好，值班经理。", "重建后的回话");

    let calls = f.calls();
    assert_eq!(calls.len(), 3, "resume 失败一次 + 重建一次：{calls:?}");
    assert!(calls[1].contains("--resume"), "第二次调用先试了续接");
    assert!(
        !calls[2].contains("--resume"),
        "重建不带 --resume（决策 424③）：{calls:?}"
    );

    let messages = f
        .store
        .list_foreman_messages(&session_id, 20, None)
        .await
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.role == "system" && m.content.contains("已用台账摘要重建")),
        "重建必须显式告知（不静默）：{messages:?}"
    );
}

#[tokio::test]
async fn first_turn_failure_is_a_turn_error_with_ledger() {
    let home = TestHome::new().unwrap();
    let store = Store::open(home.home().clone(), Arc::new(ManualClock::fixed()))
        .await
        .unwrap();
    let bin = home.scratch_dir("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("fake-claude");
    write_script(&script, "#!/bin/sh\necho 'quota exceeded' >&2\nexit 1\n");
    let runner = ForemanRunner::new(
        store.clone(),
        Settings::default(),
        home.home().clone(),
        Arc::new(NeverLlm) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    )
    .with_cli_program(script);
    let session = store
        .create_foreman_session_with_engine(FOREMAN_SESSION_KIND_TALK, "", FOREMAN_ENGINE_CLI)
        .await
        .unwrap();
    let err = runner
        .say(Some(&session.id), "在吗")
        .await
        .expect_err("首轮即失败要报错");
    assert!(err.to_string().contains("CLI"), "错误点名 CLI 引擎：{err}");
    // 失败也要留痕（决策 211④）：外框落了失败账。
    let messages = store
        .list_foreman_messages(&session.id, 20, None)
        .await
        .unwrap();
    assert!(
        messages.iter().any(|m| m.role == "system"),
        "失败轮落 system 账：{messages:?}"
    );
}
