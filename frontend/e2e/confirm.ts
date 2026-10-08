import { expect, type Locator } from '@playwright/test';

/**
 * 两步确认的第二下（票 03 / 决策 216②）。
 *
 * `destructive` 档（`合入` / `终止任务` / `重置配对`）与 `gate-skip` 档（跳过质量闸的
 * `skip`、不带自由输入的 `continue`）的钮，第一下只把该动作行就地换成后果句
 * （`.confirm-q`），同一颗钮再点一下才真提交。
 *
 * **为什么点到这些动作的用例都得走这里**：旧钉子单击一次就等状态推进 / 等请求，确认步
 * 一落地请求根本没发出去，用例以 30–180s 超时收场——症状看着像后端挂了，实则是少点了一下。
 * 这里顺带把新行为钉住：后果句必须真的出现。
 *
 * **不要用在没有确认步的动作上**（`返回修改` / `归档` / 带输入的 `继续` / `重试` /
 * `拆分任务` / `更换长上下文模型` 等）——那类钮点一下就提交，第二下会再发一次请求。
 */
export async function clickConfirmed(btn: Locator): Promise<void> {
  await expect(btn).toBeVisible();
  await btn.click();
  // 第一下之后该动作行就地多出后果句（决策 216②：12px --text-3，按钮的兄弟节点）；
  // 它出现即证明确认步真的在场，随后第二下才真提交。
  await expect(btn.locator('xpath=..').locator('.confirm-q')).toBeVisible();
  await btn.click();
}
