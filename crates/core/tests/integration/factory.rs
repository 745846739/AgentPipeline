//! 出厂技能的幂等种入与不可删（决策 261，票 foreman-operate-pipeline 02）。
//!
//! 四条播种语义（首启种入 / 改过不覆盖 / 删过补回 / 连启幂等）+ 卸载拒绝，全部骑在
//! `TestHome` 上（决策 143 接缝②）：播种是启动路径的纯文件逻辑，不需要任何替换点，
//! `TestHome` 的独占技能根就是隔离。白名单是唯一事实源——用例遍历 `FACTORY_SKILLS`
//! 而不手抄名字，新增出厂技能时这些断言自动多测一个。

use agentpipeline_core::agent::factory::{self, FACTORY_SKILLS};
use agentpipeline_core::agent::skills::{discover, SKILL_FILE};
use testkit::TestHome;

/// ① 空 home 首启：技能根里落出真 SKILL.md，且 `discover` 认得出它
/// （`GET /skills` / 目录态 / `Skill` 工具走的都是这一个发现逻辑）。
#[test]
fn factory_skill_seeds_into_an_empty_skills_root() {
    let home = TestHome::new().unwrap();
    let root = home.home().skills_dir();

    let written = factory::seed_factory_skills(&root).unwrap();
    let expected: Vec<String> = FACTORY_SKILLS.iter().map(|s| s.name.to_string()).collect();
    assert_eq!(written, expected, "种下去的恰好是白名单，一个不多一个不少");

    let found = discover(&root);
    for skill in FACTORY_SKILLS {
        let file = root.join(skill.name).join(SKILL_FILE);
        assert!(file.is_file(), "应当种出 {}", file.display());
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            content, skill.body,
            "种入的正文必须与二进制携带的逐字一致（{}）",
            skill.name
        );

        let entry = found
            .iter()
            .find(|s| s.name == skill.name)
            .unwrap_or_else(|| panic!("discover 应当认出 {}", skill.name));
        assert_eq!(
            entry.frontmatter.name.as_deref(),
            Some(skill.name),
            "frontmatter name 须与目录名一致（validate_names 的口径）"
        );
        assert!(
            entry
                .frontmatter
                .description
                .as_deref()
                .is_some_and(|d| !d.trim().is_empty()),
            "技能列表与目录态靠 description：{}",
            skill.name
        );
    }
}

/// ② 用户改过的文件一字不动——种入只补缺失。
#[test]
fn seeding_never_overwrites_a_file_the_user_modified() {
    let home = TestHome::new().unwrap();
    let root = home.home().skills_dir();
    let file = root.join(FACTORY_SKILLS[0].name).join(SKILL_FILE);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mine = "---\nname: operate-pipeline\n---\n\n用户改过的正文，一个字都不能动。\n";
    std::fs::write(&file, mine).unwrap();

    let written = factory::seed_factory_skills(&root).unwrap();
    assert!(written.is_empty(), "文件在就不该再写：{written:?}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), mine);
}

/// ③ 从磁盘上删掉 → 下次启动补回出厂正文。
#[test]
fn a_factory_skill_deleted_on_disk_is_restored_on_next_seed() {
    let home = TestHome::new().unwrap();
    let root = home.home().skills_dir();
    factory::seed_factory_skills(&root).unwrap();

    for skill in FACTORY_SKILLS {
        std::fs::remove_dir_all(root.join(skill.name)).unwrap();
    }
    let written = factory::seed_factory_skills(&root).unwrap();
    let expected: Vec<String> = FACTORY_SKILLS.iter().map(|s| s.name.to_string()).collect();
    assert_eq!(written, expected, "删掉的应当被补回");
    for skill in FACTORY_SKILLS {
        let file = root.join(skill.name).join(SKILL_FILE);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), skill.body);
    }
}

/// ④ 连启两次幂等：第二次一个字节都不写。
/// 判据是 **mtime 不变**——内容相等挡不住「同内容重写一遍」，而那正是要禁的形状。
#[test]
fn seeding_twice_writes_nothing_the_second_time() {
    let home = TestHome::new().unwrap();
    let root = home.home().skills_dir();
    factory::seed_factory_skills(&root).unwrap();

    let before: Vec<std::time::SystemTime> = FACTORY_SKILLS
        .iter()
        .map(|s| {
            root.join(s.name)
                .join(SKILL_FILE)
                .metadata()
                .unwrap()
                .modified()
                .unwrap()
        })
        .collect();

    let written = factory::seed_factory_skills(&root).unwrap();
    assert!(written.is_empty(), "第二次启动不该再写：{written:?}");
    for (skill, before) in FACTORY_SKILLS.iter().zip(before) {
        let after = root
            .join(skill.name)
            .join(SKILL_FILE)
            .metadata()
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(after, before, "{} 第二次启动被重写了", skill.name);
    }
}

/// 卸载对白名单技能拒绝，报文说清出厂技能不可删，文件原地还在。
/// 逐白名单项断言：名字取自 `FACTORY_SKILLS`，拒删的判据与播种消费的是同一份常量。
#[test]
fn uninstall_refuses_to_remove_a_factory_skill() {
    let home = TestHome::new().unwrap();
    let root = home.home().skills_dir();
    factory::seed_factory_skills(&root).unwrap();

    for skill in FACTORY_SKILLS {
        let err = agentpipeline_core::agent::skill_import::uninstall(&root, skill.name)
            .expect_err("出厂技能不该删得掉");
        let msg = err.to_string();
        assert!(msg.contains("出厂技能"), "报文要说清是出厂技能：{msg}");
        assert!(msg.contains("不可删除"), "报文要说清不可删除：{msg}");
        assert!(
            root.join(skill.name).join(SKILL_FILE).is_file(),
            "拒绝之后文件必须还在：{}",
            skill.name
        );
    }
}

// ─────────────────────── 值班长点名（决策 261⑤，票 03）───────────────────────
//
// 点名住在 foreman 阶段配置行的 `persona_append`（**配置默认值**，不是 prompt 里写死）：
// 只在无值时写、用户改过 / 清空过一律尊重——这些语义守卫的是「设置页那格是权威」。

use agentpipeline_core::pipeline::foreman::FOREMAN_STAGE_KEY;
use agentpipeline_core::types::{EnvMode, StageConfig};
use std::sync::Arc;
use testkit::ManualClock;

async fn store_for(home: &TestHome) -> agentpipeline_core::storage::Store {
    agentpipeline_core::storage::Store::open(home.home().clone(), Arc::new(ManualClock::fixed()))
        .await
        .unwrap()
}

/// 升级路径 / 首启：没有 foreman 行 → 建行，且**只**带点名（不顺手配别的字段）。
#[tokio::test]
async fn pointer_seeds_a_missing_foreman_row() {
    let home = TestHome::new().unwrap();
    let store = store_for(&home).await;
    assert!(store
        .get_stage_config(FOREMAN_STAGE_KEY)
        .await
        .unwrap()
        .is_none());

    assert!(factory::seed_foreman_pointer(&store).await.unwrap());
    let cfg = store
        .get_stage_config(FOREMAN_STAGE_KEY)
        .await
        .unwrap()
        .expect("应当建出 foreman 行");
    assert_eq!(
        cfg.persona_append.as_deref(),
        Some(factory::FOREMAN_SKILL_POINTER)
    );
    assert!(cfg.provider_id.is_none(), "只播点名，不碰别的字段");
    assert!(
        cfg.skills_json.is_none(),
        "点名不进 skills_json（决策 261④）"
    );
}

/// 有行但 `persona_append` 是 NULL → 补值，其余字段原样保留。
#[tokio::test]
async fn pointer_fills_a_null_value_and_keeps_the_rest() {
    let home = TestHome::new().unwrap();
    let store = store_for(&home).await;
    store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            env_mode: Some(EnvMode::Ask),
            persona_append: None,
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(factory::seed_foreman_pointer(&store).await.unwrap());
    let cfg = store
        .get_stage_config(FOREMAN_STAGE_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cfg.persona_append.as_deref(),
        Some(factory::FOREMAN_SKILL_POINTER)
    );
    assert_eq!(cfg.env_mode, Some(EnvMode::Ask), "其余字段一个都不动");
}

/// 用户改过 / 清空过的值永远优先：`Some("")` 是「用户显式关掉点名」，不是「没有值」。
#[tokio::test]
async fn pointer_respects_a_value_the_user_set_or_cleared() {
    for mine in ["我自己写的点名", ""] {
        let home = TestHome::new().unwrap();
        let store = store_for(&home).await;
        store
            .upsert_stage_config(&StageConfig {
                stage: FOREMAN_STAGE_KEY.to_string(),
                persona_append: Some(mine.to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert!(
            !factory::seed_foreman_pointer(&store).await.unwrap(),
            "用户值不被播种覆盖：{mine:?}"
        );
        let cfg = store
            .get_stage_config(FOREMAN_STAGE_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cfg.persona_append.as_deref(), Some(mine));
    }
}

/// 幂等：第二次启动什么都不写（返回 false、值不变）。
#[tokio::test]
async fn pointer_seeding_is_idempotent() {
    let home = TestHome::new().unwrap();
    let store = store_for(&home).await;
    assert!(
        factory::seed_foreman_pointer(&store).await.unwrap(),
        "首播要写"
    );
    assert!(
        !factory::seed_foreman_pointer(&store).await.unwrap(),
        "二播不写"
    );
    let cfg = store
        .get_stage_config(FOREMAN_STAGE_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cfg.persona_append.as_deref(),
        Some(factory::FOREMAN_SKILL_POINTER)
    );
}
