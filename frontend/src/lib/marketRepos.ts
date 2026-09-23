/**
 * 市场仓名单编辑器的判据（决策 194；收口见票 23 / 决策 250 Q2）：归一 / 校验 / 增删。
 *
 * **后端是权威**：这里只做「按下按钮之前就能看见的错」——非法的 `owner/repo`、重复项。
 *
 * **两边的关系是有方向的一对**：前端**先归一、再校验**，送出去的是归一后的 `owner/repo`；
 * 故不变式是「[`normalizeRepo`] 的输出**必须**是 `crates/core/src/agent/repo.rs::RepoId`
 * 接受的输入的**子集**」。这条子集不变量**不再是注释里的承诺**：它由共享表
 * `tests/fixtures/repo_id.json` 机器钉住（票 23，照决策 246 `host_policy_loopback.json`
 * 先例）——本文件的归一/校验由 `marketReposFixture.test.ts` 逐行对着表断言，后端
 * `RepoId::parse` 由 `repo.rs` 的表测试断言**表里的归一形态它都认识**。两侧同表、同一
 * 断言方向，任何一侧改了规范另一侧没跟就变红。举例：`www.github.com` 前缀**是因为归一里
 * 抹掉了**才成立——`RepoId` 只认 `github.com` 那族，抹掉它正是为了让两边对得上，
 * 共享表的 `www` 行钉的就是这一步。
 *
 * **为什么不校验「这个仓存在」**：那要打网络，而「不在名单里点添加之前一个字节都不下载」
 * 是本 effort 的硬约束（票 03）。存在的判定只有后端在 `head()` 时做，失败类别是
 * `repo_not_found` / `repo_unreadable`。
 */

/**
 * 归一化一个仓名：trim、去掉常见的粘贴前缀、去掉 `.git` 后缀与尾斜杠。
 *
 * **刻意不小写**（与老的 `normalizeSource` 不同）：GitHub 的 `owner/repo` 大小写不敏感，
 * 但展示意义真实存在——用户从地址栏复制来的是 `Obra/Superpowers`，把它改写成
 * `obra/superpowers` 等于在界面上显示一个用户没输入过的字符串（决策 194 的信任单元是
 * 用户认得出的那个仓名）。大小写只在**去重**时被忽略（见 [`addRepo`]），保留原样。
 *
 * 前缀只抹 `github.com` 那一族：用户会直接从浏览器地址栏粘贴。别的 scheme 一律留着，
 * 让 [`validateRepo`] 明确报「不要带 scheme」——悄悄抹掉非 GitHub 的主机会让用户以为
 * 自己放行的是别处的仓。
 */
export function normalizeRepo(raw: string): string {
  let value = raw.trim();
  if (value === '') return '';
  value = value.replace(/^(?:https?:\/\/)?(?:www\.)?github\.com\//i, '');
  value = value.replace(/\/+$/, '');
  value = value.replace(/\.git$/i, '');
  value = value.replace(/\/+$/, '');
  return value;
}

/**
 * 校验一个仓名；返回面向用户的错误说明，`null` = 合法。
 *
 * 规则逐条与后端 `RepoId` 对齐（**规范不能有第二个版本**——决策 187 的原话；实现有两份
 * 是被迫的，对齐由共享表 `tests/fixtures/repo_id.json` 双侧断言钉住，票 23）：
 * 恰好一个 `/`、两段都非空、只含 ASCII 字母数字与 `-` `_` `.`、不以 `.` 或 `-` 开头、
 * 拒绝带 scheme、拒绝含 `@`、拒绝含 `..`、拒绝多余斜杠或空段、拒绝非 ASCII。
 *
 * 为什么这么严：归一后的字符串会被拼进 URL（`{base}/{owner}/{repo}.git`）。libgit2 的传输
 * 注册表里 `git://` / `file://` / `ssh://` 都在，**裸文件系统路径也会被 local transport 吃掉**，
 * 故「用户填的字符串」不能有机会变成 URL 的形态——`..` 与多余斜杠更是直接指向路径穿越。
 */
export function validateRepo(raw: string): string | null {
  const value = normalizeRepo(raw);
  if (value === '') return '请填写仓名。';
  if (value.includes('://')) {
    return '只填 owner/repo，不要带 scheme（粘 GitHub 网址可以，github.com 前缀会被去掉）。';
  }
  if (value.includes('@')) {
    return '仓名里不能有 @：SSH 写法（git@github.com:owner/repo）不支持，请填 owner/repo。';
  }
  if (/[^\x20-\x7e]/.test(value)) {
    return '仓名只能是 ASCII 字符，不要用中文或全角字符。';
  }
  const parts = value.split('/');
  if (parts.length !== 2) {
    return '仓名要恰好是 owner/repo 两段，不带多余斜杠、路径或查询。';
  }
  const owner = parts[0] ?? '';
  const name = parts[1] ?? '';
  if (owner === '' || name === '') return 'owner 与 repo 都不能为空。';
  if (value.includes('..')) return '仓名里不能出现 ..（那会被当成路径）。';
  if (owner.startsWith('.') || owner.startsWith('-')) return 'owner 不能以 . 或 - 开头。';
  if (name.startsWith('.') || name.startsWith('-')) return 'repo 不能以 . 或 - 开头。';
  if (!/^[A-Za-z0-9._-]+$/.test(owner) || !/^[A-Za-z0-9._-]+$/.test(name)) {
    return '仓名只能包含 ASCII 字母、数字与 - _ .';
  }
  return null;
}

/**
 * 追加一个仓；非法或重复 → `null`（调用方给出提示），合法 → 新列表。
 *
 * **去重按小写比**：GitHub 认的是同一个仓，`Obra/Superpowers` 与 `obra/superpowers`
 * 放进名单两行没有任何意义（放行判定本来就是按仓，不是按拼写）。归一里不抹大小写是
 * 为了展示，这里抹是为了判定——两件事各按各的口径，别混。
 */
export function addRepo(repos: string[], raw: string): string[] | null {
  const value = normalizeRepo(raw);
  if (validateRepo(value) !== null) return null;
  const lower = value.toLowerCase();
  if (repos.some((r) => r.toLowerCase() === lower)) return null;
  return [...repos, value];
}

/** 移除一个仓（按列表里那一行的原样值，精确匹配）。 */
export function removeRepo(repos: string[], value: string): string[] {
  return repos.filter((r) => r !== value);
}
