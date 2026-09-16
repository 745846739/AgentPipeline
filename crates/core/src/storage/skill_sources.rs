//! 技能的**来源记录**存储（决策 194）：一个已装技能是从哪个仓的哪个 commit 装来的。
//!
//! 表是 `skill_sources`（迁移 0011），一行一个技能。它承担三件事，逐条写在迁移文件的注释里：
//! 同名冲突的报文、一键安装的「跳过下载」判定（决策 181⑦ 被决策 194 修订）、卸载时一并清掉。
//!
//! **它不是安全判定**：放行与否只看 `owner/repo` 在不在仓名单里
//! （[`crate::agent::repo::repo_allowed`]）。这张表是只读的记述，删掉不影响任何放行判断。

use super::Store;
use crate::Result;

/// 一行来源记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSource {
    /// 技能名（唯一身份，决策 172）。
    pub name: String,
    pub owner: String,
    pub repo: String,
    /// 完整 40 位 commit SHA（**比对用**；短 SHA 只用于显示）。
    pub commit_sha: String,
    /// 技能目录在仓根内的相对路径（仓根技能是空串）。
    pub subpath: String,
    pub installed_at: String,
}

impl SkillSource {
    /// 归一形态 `owner/repo`。
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// 报文案用的短坐标：`owner/repo@<短 SHA>:<子路径>`（决策 194 规定的形态）。
    ///
    /// 短 SHA 取前 7 位；子路径为空（技能在仓根）时写「仓根」而不是留一个空荡荡的冒号。
    pub fn describe(&self) -> String {
        let short: String = self.commit_sha.chars().take(7).collect();
        let subpath = if self.subpath.is_empty() {
            "仓根"
        } else {
            self.subpath.as_str()
        };
        format!("{}@{short}:{subpath}", self.slug())
    }

    /// 这一份是不是**清单指的那个仓的那个目录**（不比 commit）。
    ///
    /// 一键安装那条「本地已装就跳过下载」的分支靠它。**为什么不比 commit**：推荐清单是一个
    /// **指针**（仓 + 目录），不是一份带版本的名录——它若钉死 commit，那份常量就会随上游漂移
    /// 变成陈旧数据；而"和当前 tip 比"又要为此多发一次网络请求，正好毁掉这条分支存在的理由
    /// （决策 181⑦：未配来源时也走得通，因为一次网络请求都不发生）。
    ///
    /// 跳过时**原样保留**记录里的 commit，所以两种沉默都不会发生：不静默换旧版（没换），
    /// 也不静默升级（没升）。要换另一份，用户得显式带 `overwrite`。
    pub fn matches_slug_and_path(&self, repo_slug: &str, subpath: &str) -> bool {
        self.slug().eq_ignore_ascii_case(repo_slug) && self.subpath == subpath
    }
}

impl Store {
    /// 记下一个技能来自哪里（安装成功后写）。
    ///
    /// 同一个技能重装（覆盖）时**改写**这一行：一行一技能是不变式，历史来源不留档
    /// （要的是「现在这份是从哪来的」，不是审计流水）。
    pub async fn record_skill_source(&self, source: &SkillSource) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO skill_sources (name, owner, repo, commit_sha, subpath, installed_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET owner = excluded.owner, repo = excluded.repo, \
                                             commit_sha = excluded.commit_sha, \
                                             subpath = excluded.subpath, \
                                             installed_at = excluded.installed_at",
        )
        .bind(&source.name)
        .bind(&source.owner)
        .bind(&source.repo)
        .bind(&source.commit_sha)
        .bind(&source.subpath)
        .bind(&source.installed_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 查一个技能来自哪里；没有记录（手工拷进来、本地导入、扫描进来的）→ `None`。
    ///
    /// `None` 是正常状态，不是错误：冲突报文此时回落到「技能根下的路径」那个形态。
    pub async fn skill_source(&self, name: &str) -> Result<Option<SkillSource>> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT name, owner, repo, commit_sha, subpath, installed_at \
             FROM skill_sources WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(
            |(name, owner, repo, commit_sha, subpath, installed_at)| SkillSource {
                name,
                owner,
                repo,
                commit_sha,
                subpath,
                installed_at,
            },
        ))
    }

    /// 忘掉一个技能的来源记录（卸载时一并删）。返回是否真的删掉一行。
    ///
    /// **不删的话**：下一次同名安装的冲突报文会报一个已经不存在的技能曾经从哪儿来。
    /// 停用 / 启用（阶段配置里的引用）与记录无关，不联动。
    pub async fn forget_skill_source(&self, name: &str) -> Result<bool> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query("DELETE FROM skill_sources WHERE name = ?")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(rows > 0)
    }
}
