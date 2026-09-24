//! 出厂默认（决策 261，窄口修订决策 172①）：白名单技能的幂等种入 + 值班长点名。
//!
//! 二进制只携带 [`FACTORY_SKILLS`] 白名单内的技能正文（首期仅 `operate-pipeline` 一条），
//! **白名单常量是播种与拒删两处消费的唯一事实源**——`seed_factory_skills` 往技能根里补它，
//! [`crate::agent::skill_import::uninstall`] 按它拒绝卸载。
//!
//! 播种语义是「只补缺失」的幂等写：文件在则一字不动（用户改过不覆盖），被从磁盘删掉则下次
//! 启动补回，连启两次第二次一个字节都不写。172①「二进制不夹带技能正文挤兑技能根」的本意
//! 不变——正文只此一份、且为本仓自撰。
//!
//! [`seed_foreman_pointer`] 播种的是 foreman 阶段配置行 `persona_append` 里**一句指针**
//! （遇操作类指令先按名拉手册），不是技能正文：决策 182「值班长 prompt 不注入技能」的立场
//! 不变，正文仍由 `Skill` 工具按需拉取。指针是**配置默认值**——只在无值时写，用户改过 /
//! 清空过一律尊重（设置页可见、可编辑、可关）。

use std::path::Path;

use crate::error::Result;
use crate::home::{restrict_file_permissions, restrict_permissions};
use crate::pipeline::foreman::FOREMAN_STAGE_KEY;
use crate::storage::Store;
use crate::types::StageConfig;

/// 一条出厂技能：名字 + 随二进制携带的完整 `SKILL.md` 正文。
pub struct FactorySkill {
    /// 技能名（同时是技能根下的目录名，frontmatter `name` 须与它一致）。
    pub name: &'static str,
    /// `SKILL.md` 的逐字内容（含 frontmatter）。
    pub body: &'static str,
}

/// 出厂技能白名单（决策 261 的边界本身）：首期仅 `operate-pipeline` 一条。
///
/// 播种与拒删都只认这份常量。往里加名字 = 往二进制里加一份技能正文 = 扩大对 172① 的修订
/// 面——那必须先改决策 261 的行，再改这里。
pub const FACTORY_SKILLS: &[FactorySkill] = &[FactorySkill {
    name: "operate-pipeline",
    body: include_str!("factory/operate-pipeline/SKILL.md"),
}];

/// 这个名字是不是出厂技能（卸载拒绝的判据，与播种同一份白名单）。
pub fn is_factory_skill(name: &str) -> bool {
    FACTORY_SKILLS.iter().any(|s| s.name == name)
}

/// foreman 阶段配置行 `persona_append` 的出厂默认值：一句点名指针（决策 261⑤）。
///
/// 大意是「遇操作类指令先按名拉 `operate-pipeline` 手册」。种进配置而不是写死在 prompt
/// 组装里：设置页看得见、改得动、关得掉，且用户改过的值永远优先。
pub const FOREMAN_SKILL_POINTER: &str =
    "涉及流水线操作的指令（推进、重试、拍板、合入、批量处置、查任务状态），先用 Skill 工具按名拉取 operate-pipeline 手册，按手册执行。";

/// [`seed_factory_defaults`] 的读数：什么写了、什么没写、什么失败了。
#[derive(Debug, Default)]
pub struct SeedReport {
    /// 本次真正种入的技能名（空 = 都在，没动任何一个文件）。
    pub skills_written: Vec<String>,
    /// foreman 点名是否本次写入（false = 无值可补的路径没有，或用户值原样保留）。
    pub pointer_written: bool,
    /// 非致命失败（技能文件写不进去）：记下来让调用方告警，不阻断启动——
    /// 不声明该技能时 `Skill` 工具对缺失名字有友好回落（决策 261④ 的同一笔账）。
    pub warnings: Vec<String>,
}

/// 把白名单技能幂等种进技能根：只补缺失，已存在的文件（哪怕被改过）一字不动。
///
/// 返回本次真正写入的名字。写入只收紧**自己新建**的技能目录与文件，不碰技能根本身
/// （`[skills] dir` 可能指向用户自己维护的生态目录，决策 172 的同一边界）。
pub fn seed_factory_skills(skills_root: &Path) -> Result<Vec<String>> {
    let mut written = Vec::new();
    for skill in FACTORY_SKILLS {
        let dir = skills_root.join(skill.name);
        let file = dir.join(crate::agent::skills::SKILL_FILE);
        if file.is_file() {
            continue; // 只补缺失：用户改过的、删没删的、上次种过的，全都不动
        }
        std::fs::create_dir_all(&dir).map_err(|e| {
            crate::error::Error::Config(format!(
                "出厂技能 {} 的目录建不出来（{}）：{e}",
                skill.name,
                dir.display()
            ))
        })?;
        std::fs::write(&file, skill.body).map_err(|e| {
            crate::error::Error::Config(format!(
                "出厂技能 {} 种入失败（{}）：{e}",
                skill.name,
                file.display()
            ))
        })?;
        restrict_permissions(&dir);
        restrict_file_permissions(&file);
        written.push(skill.name.to_string());
    }
    Ok(written)
}

/// 给 foreman 阶段配置行播种点名指针：**只在无值时写**。
///
/// 三种落点：无行 → 建行（只带 `persona_append`，其余字段全缺省）；有行但
/// `persona_append` 是 `NULL` → 补值、其余字段原样保留；已有值（含用户显式清空的
/// `Some("")`）→ 一个字不动。返回是否本次写入。
pub async fn seed_foreman_pointer(store: &Store) -> Result<bool> {
    let existing = store.get_stage_config(FOREMAN_STAGE_KEY).await?;
    let cfg = match existing {
        None => StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            persona_append: Some(FOREMAN_SKILL_POINTER.to_string()),
            updated_at: store.now(),
            ..Default::default()
        },
        Some(mut cfg) if cfg.persona_append.is_none() => {
            cfg.persona_append = Some(FOREMAN_SKILL_POINTER.to_string());
            cfg.updated_at = store.now();
            cfg
        }
        // 有值——包括用户清空成 `Some("")`：那是用户的选择，尊重它。
        Some(_) => return Ok(false),
    };
    store.upsert_stage_config(&cfg).await?;
    Ok(true)
}

/// 启动时的出厂播种（决策 261）：技能文件 + 值班长点名，一次调用、两处幂等。
///
/// 调用点在 `serve()` 的 `validate_startup` **之前**：用户若在阶段配置里声明了出厂技能，
/// 校验要看到文件已经就位。技能文件的写失败进 [`SeedReport::warnings`]（不阻断启动），
/// 点名的数据库写失败向上传播（库都打开了还写不进，后面处处会炸）。
pub async fn seed_factory_defaults(home: &crate::home::Home, store: &Store) -> Result<SeedReport> {
    let mut report = SeedReport::default();
    match seed_factory_skills(&home.skills_dir()) {
        Ok(written) => report.skills_written = written,
        Err(e) => report.warnings.push(e.to_string()),
    }
    report.pointer_written = seed_foreman_pointer(store).await?;
    Ok(report)
}
