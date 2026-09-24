//! 提议的存储与生命周期（决策 188 / 207，票 02）。
//!
//! 本票**一个写工具都不加**（写工具在票 04 / 05 / 06），故这里验的是提议层自己的规则：
//! 四个状态、一次一按的占用、过期不删行、态势指纹的漂移判定。生成路径（写工具 →
//! 执行点拦截 → 提议）各自跟着加工具的那张票走。

use agentpipeline_core::clock::Clock;
use agentpipeline_core::pipeline::foreman::{situation_drift, situation_fingerprint};
use agentpipeline_core::storage::proposals::{
    ForemanProposalStatus, NewForemanProposal, FOREMAN_PROPOSAL_TTL_MINUTES,
};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::TaskStatus;
use serde_json::json;
use testkit::{seed_project, seed_task, ManualClock, TestHome};

struct Fixture {
    _home: TestHome,
    store: Store,
    clock: ManualClock,
    session_id: String,
}

async fn fixture() -> Fixture {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
    let session_id = store.create_foreman_session("夜班").await.unwrap().id;
    Fixture {
        _home: home,
        store,
        clock,
        session_id,
    }
}

impl Fixture {
    async fn propose(&self, tool: &str, args: serde_json::Value) -> String {
        self.store
            .create_foreman_proposal(NewForemanProposal {
                kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
                payload: None,
                session_id: self.session_id.clone(),
                tool: tool.to_string(),
                args,
                summary: format!("（用例）{tool}"),
                situation: None,
            })
            .await
            .unwrap()
            .id
    }

    async fn status(&self, id: &str) -> String {
        self.store
            .get_foreman_proposal(id)
            .await
            .unwrap()
            .unwrap()
            .status
            .as_str()
            .to_string()
    }
}

/// 有效期由**存储层**按决策 207 的 10 分钟给，不由调用方各写一遍。
#[tokio::test]
async fn proposal_ttl_is_ten_minutes_from_creation() {
    let fx = fixture().await;
    let id = fx.propose("write_file", json!({"path": "a.md"})).await;
    let p = fx.store.get_foreman_proposal(&id).await.unwrap().unwrap();
    assert_eq!(
        (p.expires_at - p.created_at).num_minutes(),
        FOREMAN_PROPOSAL_TTL_MINUTES
    );
    assert_eq!(p.status, ForemanProposalStatus::Pending);
    assert!(!p.is_expired(fx.store.now()));
    fx.clock.advance_secs(FOREMAN_PROPOSAL_TTL_MINUTES * 60 + 1);
    assert!(p.is_expired(fx.store.now()));
}

/// 一次一按：占用是原子的，第二个占用者拿不到东西。
#[tokio::test]
async fn claiming_is_the_one_press_rule() {
    let fx = fixture().await;
    let id = fx.propose("write_file", json!({"path": "a.md"})).await;

    let first = fx.store.claim_foreman_proposal(&id).await.unwrap();
    assert!(first.is_some(), "第一个占用者应拿到提议");
    assert!(first.unwrap().claimed_at.is_some());
    assert!(
        fx.store
            .claim_foreman_proposal(&id)
            .await
            .unwrap()
            .is_none(),
        "第二个占用者拿不到——这就是「不可重放」"
    );

    // 执行失败：释放占用、退回未决，人还能再按一次。
    fx.store.release_foreman_proposal(&id).await.unwrap();
    assert_eq!(fx.status(&id).await, "pending");
    assert!(fx
        .store
        .claim_foreman_proposal(&id)
        .await
        .unwrap()
        .is_some());
}

/// 终态只有一个方向：落到 executed / rejected 之后不能再被占用。
#[tokio::test]
async fn a_resolved_proposal_cannot_be_claimed_again() {
    let fx = fixture().await;
    for (final_status, expected) in [
        (ForemanProposalStatus::Executed, "executed"),
        (ForemanProposalStatus::Rejected, "rejected"),
    ] {
        let id = fx.propose("write_file", json!({"path": "a.md"})).await;
        fx.store.claim_foreman_proposal(&id).await.unwrap().unwrap();
        let resolved = fx
            .store
            .resolve_foreman_proposal(&id, final_status)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.status.as_str(), expected);
        assert!(resolved.resolved_at.is_some());
        assert!(resolved.claimed_at.is_none(), "落终态时占用一并清掉");
        assert!(
            fx.store
                .claim_foreman_proposal(&id)
                .await
                .unwrap()
                .is_none(),
            "已决的提议不可再执行"
        );
        assert!(
            fx.store
                .resolve_foreman_proposal(&id, ForemanProposalStatus::Executed)
                .await
                .unwrap()
                .is_none(),
            "重复落终态是空操作，不覆盖第一次的时间戳"
        );
    }
}

/// 没被占用的提议落不了终态——「人按下」这条路径不能绕过去。
#[tokio::test]
async fn resolving_without_a_claim_does_nothing() {
    let fx = fixture().await;
    let id = fx.propose("write_file", json!({"path": "a.md"})).await;
    assert!(fx
        .store
        .resolve_foreman_proposal(&id, ForemanProposalStatus::Executed)
        .await
        .unwrap()
        .is_none());
    assert_eq!(fx.status(&id).await, "pending");
}

/// 过期清扫**改状态不删行**（决策 207：那一轮留在时间线里），且不碰正在执行的那条。
#[tokio::test]
async fn the_sweep_expires_rows_without_deleting_them() {
    let fx = fixture().await;
    let stale = fx.propose("write_file", json!({"path": "a.md"})).await;
    let in_flight = fx.propose("write_file", json!({"path": "b.md"})).await;
    // 一条正好被人按着（已占用）：它不该被清扫改成过期——那次执行要自己落终态。
    fx.store
        .claim_foreman_proposal(&in_flight)
        .await
        .unwrap()
        .unwrap();

    fx.clock.advance_secs(FOREMAN_PROPOSAL_TTL_MINUTES * 60 + 1);
    let swept = fx
        .store
        .expire_foreman_proposals(fx.store.now())
        .await
        .unwrap();
    assert_eq!(swept, 1, "只扫到没被占用的那条");

    assert_eq!(fx.status(&stale).await, "expired");
    assert_eq!(fx.status(&in_flight).await, "pending");
    // 行还在（不删行才有「过期只让按钮变灰、那一轮留在时间线」）
    let all = fx
        .store
        .list_foreman_proposals(&fx.session_id, 50)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    // 升序钉的是**排序本身**，不拿「位置 0」认行：id 是 ULID，同一毫秒内大小由随机段
    // 决定、与插入先后无关——「先建的那条排第一」在全量套件里实测偶发不成立。
    assert!(
        all[0].id < all[1].id,
        "时间线按 id 升序给出全部（含已过期）"
    );
    let row = |id: &str| all.iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        row(stale.as_str()).status,
        ForemanProposalStatus::Expired,
        "过期的那条仍在时间线里（不删行）"
    );
    assert_eq!(
        row(in_flight.as_str()).status,
        ForemanProposalStatus::Pending
    );
    // 未决清单里只剩那条在执行的
    let pending = fx
        .store
        .list_pending_foreman_proposals(&fx.session_id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, in_flight);
}

/// 单条过期：与清扫分开的一条路（按「执行」时才发现过期），同样不抢占用。
#[tokio::test]
async fn expiring_one_proposal_leaves_claimed_ones_alone() {
    let fx = fixture().await;
    let id = fx.propose("write_file", json!({"path": "a.md"})).await;
    fx.clock.advance_secs(FOREMAN_PROPOSAL_TTL_MINUTES * 60 + 1);

    fx.store.claim_foreman_proposal(&id).await.unwrap().unwrap();
    assert!(
        fx.store
            .expire_foreman_proposal(&id)
            .await
            .unwrap()
            .is_none(),
        "正在执行的那条不改成过期"
    );
    fx.store.release_foreman_proposal(&id).await.unwrap();
    let expired = fx
        .store
        .expire_foreman_proposal(&id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired.status, ForemanProposalStatus::Expired);
    assert!(expired.resolved_at.is_some());
}

/// 年龄清理与对讲台对话同口径：到点删行（那时时间线里也没有这一轮了）。
#[tokio::test]
async fn old_proposals_are_purged_by_age() {
    let fx = fixture().await;
    fx.propose("write_file", json!({"path": "a.md"})).await;
    // 清理的判据是 `created_at < cutoff`：把截止点推进一秒，旧那条才真的落后于它。
    fx.clock.advance_secs(1);
    let cutoff = fx.store.now();
    fx.clock.advance_secs(31 * 24 * 3600);
    let fresh = fx.propose("write_file", json!({"path": "b.md"})).await;

    let purged = fx.store.purge_foreman_proposals(cutoff).await.unwrap();
    assert_eq!(purged, 1);
    let left = fx
        .store
        .list_foreman_proposals(&fx.session_id, 50)
        .await
        .unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, fresh);
}

/// 提议挂在会话上：另一班的读法拿不到它（决策 204 的兑现点）。
#[tokio::test]
async fn proposals_are_scoped_to_their_session() {
    let fx = fixture().await;
    fx.propose("write_file", json!({"path": "a.md"})).await;
    let other = fx.store.create_foreman_session("另一班").await.unwrap().id;
    assert!(fx
        .store
        .list_foreman_proposals(&other, 50)
        .await
        .unwrap()
        .is_empty());
    assert!(fx
        .store
        .list_pending_foreman_proposals(&other)
        .await
        .unwrap()
        .is_empty());
}

// ─────────────────────────── 态势指纹（拒执判据）───────────────────────────

/// 指纹带的是**会让人改变主意**的三样：状态、工位、后端此刻下发的动作集。
#[tokio::test]
async fn the_fingerprint_carries_status_stage_and_actions() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    seed_project(&store, "p1", "示例", home.path(), "main")
        .await
        .unwrap();
    seed_task(&store, "t1", "p1").await.unwrap();

    let before = situation_fingerprint(&store, "t1").await.unwrap();
    assert_eq!(before["task_id"], "t1");
    assert!(before["status"].is_string());
    assert!(before["stage"].is_string());
    assert!(before["allowed_actions"].is_array());
    // 与后取的一份相等 → 没有漂移
    let again = situation_fingerprint(&store, "t1").await.unwrap();
    assert!(situation_drift(&before, &again).is_none());

    // 查无此任务也是一份合法指纹（模型可能提了一个后来被删掉的任务）
    let missing = situation_fingerprint(&store, "nope").await.unwrap();
    assert_eq!(missing["missing"], true);
    assert!(
        situation_drift(&before, &missing).is_some(),
        "从「有这个任务」到「没有」是最该被拒的一种漂移"
    );
}

/// 状态一变，指纹就不等——这是「现在的情况已经不是它当时说的那样」的判据本身。
#[tokio::test]
async fn a_status_change_shows_up_as_drift() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    seed_project(&store, "p1", "示例", home.path(), "main")
        .await
        .unwrap();
    seed_task(&store, "t1", "p1").await.unwrap();
    let before = situation_fingerprint(&store, "t1").await.unwrap();

    store
        .set_task_status("t1", TaskStatus::Cancelled)
        .await
        .unwrap();

    let after = situation_fingerprint(&store, "t1").await.unwrap();
    let drift = situation_drift(&before, &after).expect("状态变了就该报出漂移");
    assert!(drift.contains("任务状态"), "要说清变的是哪一样：{drift}");
}

/// 漂移的一句话说明**点名变了什么**：只说「情况变了」会让人再查一遍台账。
#[test]
fn the_drift_message_names_what_changed() {
    let base = json!({
        "task_id": "t1",
        "status": "pending",
        "stage": "develop",
        "node": "execute",
        "allowed_actions": ["resume", "cancel"],
    });
    let action_changed = json!({
        "task_id": "t1",
        "status": "pending",
        "stage": "develop",
        "node": "execute",
        "allowed_actions": ["skip"],
    });
    let drift = situation_drift(&base, &action_changed).unwrap();
    assert!(drift.contains("可按下的事"), "{drift}");
    assert!(drift.contains("resume / cancel"), "{drift}");

    let stage_changed = json!({
        "task_id": "t1",
        "status": "pending",
        "stage": "test",
        "node": "execute",
        "allowed_actions": ["resume", "cancel"],
    });
    let drift = situation_drift(&base, &stage_changed).unwrap();
    assert!(drift.contains("当前工位"), "{drift}");

    // 完全一样 → 没有漂移（拒执不该因为一次无关的重读而触发）
    assert!(situation_drift(&base, &base.clone()).is_none());
}

/// 提议的参数原样存取：执行要按它走既有端点，**提议层不解释参数**。
#[tokio::test]
async fn args_and_situation_round_trip() {
    let fx = fixture().await;
    let args = json!({"task_id": "t1", "action": "resume", "note": "带中文与换行\n第二行"});
    let situation = json!({"task_id": "t1", "status": "pending"});
    let p = fx
        .store
        .create_foreman_proposal(NewForemanProposal {
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            session_id: fx.session_id.clone(),
            tool: "task_action".into(),
            args: args.clone(),
            summary: "恢复 t1".into(),
            situation: Some(situation.clone()),
        })
        .await
        .unwrap();
    let read = fx.store.get_foreman_proposal(&p.id).await.unwrap().unwrap();
    assert_eq!(read.args, args);
    assert_eq!(read.situation, Some(situation));
    assert_eq!(read.summary, "恢复 t1");
    assert!(
        read.situation.is_some(),
        "有态势的提议与没态势的提议在库里分得开"
    );

    // 没有态势的（文件 / 命令那一类）读回来也是 None，不是空对象
    let plain = fx.propose("write_file", json!({"path": "a.md"})).await;
    assert!(fx
        .store
        .get_foreman_proposal(&plain)
        .await
        .unwrap()
        .unwrap()
        .situation
        .is_none());
}

/// 清理用的时钟是不可逆的：`Store::now()` 是唯一时间源（决策 64 / 143）。
#[tokio::test]
async fn proposal_timestamps_come_from_the_store_clock() {
    let fx = fixture().await;
    fx.clock.advance_secs(3_600);
    let id = fx.propose("write_file", json!({"path": "a.md"})).await;
    let p = fx.store.get_foreman_proposal(&id).await.unwrap().unwrap();
    assert_eq!(p.created_at, fx.clock.now());
    assert_eq!(
        (p.created_at - ManualClock::fixed().now()).num_seconds(),
        3_600
    );
}
