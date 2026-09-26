/**
 * 「这台设备还没配对」时，把**这次启动的地址**摊开（决策 285）。
 *
 * 为什么需要它：主屏图标的启动地址决定配对令牌能不能递进来（决策 191——iOS 的主屏
 * web app 与 Safari **各有独立存储**，令牌只能挂在地址上），而 standalone 窗口**没有
 * 地址栏**。于是「图标没带上参数」这种报告在界面上完全不可见：使用者只看到一句
 * 「这台设备还没配对」，排障的人（含几轮之后的自己）只能猜。这一行把猜的地方变成读的地方。
 *
 * 三种来源各自对应一条不同的修法，所以判定要分得开：
 * **地址里带着**（服务端不认 → 令牌重置过 / 这条地址旧了，重扫或重新添加）、
 * **只有本机存的那份**（这份旧了）、
 * **两处都没有**（图标是从一条裸地址添加出来的 → 必须从带令牌的那条地址重新添加）。
 *
 * 令牌值一律打码：排障要的是「地址里有没有」，不是「是什么」——把凭据印在屏幕上，
 * 一张截图就又多泄一次（191 的残余风险已经由「地址栏里有令牌」认过一次，不再加一处）。
 */

/** 把地址里的 `pair=` 值打码，其余原样（`pair` 缺省时原样返回）。 */
export function maskPairValue(href: string): string {
  return href.replace(/([?&]pair=)[^&#]*/i, '$1•••');
}

/**
 * 地址里的**查询串**（不含 hash）。判定只认这一段：应用读的是 `location.search`
 * （`capturePairingFromLocation`），所以 `#/talk?pair=…` 那种写法**递不进来**——
 * 若把它也算成「地址里带着」，就会把「图标从裸地址添加」误诊成「令牌旧了」，
 * 把人送去重扫一个本来就对的码。
 */
function searchOf(href: string): string {
  const beforeHash = href.split('#')[0] ?? '';
  const at = beforeHash.indexOf('?');
  return at === -1 ? '' : beforeHash.slice(at);
}

/** 令牌的来源：地址里带着 / 只有本机存的一份 / 两处都没有。 */
export type PairingLaunchVerdict = 'url' | 'stored' | 'none';

export function pairingLaunchVerdict(href: string, stored: string | null): PairingLaunchVerdict {
  if (new URLSearchParams(searchOf(href)).get('pair')?.trim()) return 'url';
  return stored ? 'stored' : 'none';
}

/** 给使用者看的一句话：为什么这次没能配对。 */
export function pairingLaunchNote(href: string, stored: string | null): string {
  switch (pairingLaunchVerdict(href, stored)) {
    case 'url':
      return '地址里带着配对令牌，但服务端不认——多半是令牌已重置，或这条地址是旧的';
    case 'stored':
      return '地址里没有配对令牌，本机存的那一份服务端也不认（旧了）';
    default:
      return '地址里没有配对令牌，本机也没存过——这个图标是从一条不带令牌的地址添加的';
  }
}
