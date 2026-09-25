import type { NotifyChannelPayload, NotifySettings } from '../api/types';

/**
 * 「离线通知」设置页的判据（决策 272⑥⑦⑧）。
 *
 * **后端是权威**：这里只做「按下按钮之前就能看见的错」——缺必填件、端点没带 scheme；
 * 开启时 BlueBubbles 够不够得着由后端 ping（够不着不当成功），前端只把报文接住。
 *
 * 两级关系（决策 272⑥）：通道四件（类型 + 端点 + password + 收件人）作为**一个整体**
 * 覆盖 `config.toml`——表单里只有当前选中的通道那几件会送出去，后端也按整体收，
 * 「界面指向 BlueBubbles 而配置说 feishu」因此没有藏身之处。
 *
 * 秘密面（决策 112 范式）：读回只给常量掩码 `***`；掩码或留空 = 沿用已存值——
 * 载荷里**省略**该字段，绝不把掩码当值回传。
 */

/** 读接口回显的掩码（`lib/providers.ts::API_KEY_MASK` 同一个值）。 */
export const NOTIFY_SECRET_MASK = '***';

export type NotifyChannelKind = 'generic' | 'feishu' | 'bluebubbles';

/** 通道的界面名。值域与后端 `NotifyFormat` 的 serde 小写形一致。 */
export const CHANNEL_LABELS: Record<NotifyChannelKind, string> = {
  generic: '通用 webhook',
  feishu: '飞书机器人',
  bluebubbles: 'iMessage（BlueBubbles）',
};

/**
 * 「这份通道是谁定的」那句话（决策 194 在仓名单上立的规矩；中文标签照
 * `lib/sharePairing.ts::bindSourceLabel` 的口径）。
 */
export function notifyOriginLabel(origin: string | null | undefined): string {
  if (origin === 'settings') return '界面上的选择';
  return '配置文件';
}

/** 编辑态草稿：秘密以读接口回显值（`***` 或空）预填。 */
export interface NotifyDraft {
  channel: NotifyChannelKind;
  webhookUrl: string;
  bluebubblesUrl: string;
  bluebubblesPassword: string;
  bluebubblesRecipient: string;
}

export function draftFromSettings(s: NotifySettings): NotifyDraft {
  return {
    channel: s.channel ?? 'generic',
    webhookUrl: s.webhook_url ?? '',
    bluebubblesUrl: s.bluebubbles_url ?? '',
    bluebubblesPassword: s.bluebubbles_password ?? '',
    bluebubblesRecipient: s.bluebubbles_recipient ?? '',
  };
}

/** 端点形状（`lib/providers.ts::isUsableBaseUrl` 同一把尺）：必须 http(s):// 开头且带主机。 */
export function isUsableEndpoint(raw: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return false;
  }
  return (parsed.protocol === 'http:' || parsed.protocol === 'https:') && parsed.hostname !== '';
}

/**
 * 表单校验；返回面向用户的错误，`null` = 通过。
 *
 * 只校验**当前选中通道**的必填件（后端按整体收，别通道的字段不送也不查）。
 * `***` 在 password 上是合法通过项——它表示「沿用已存值」；若其实没有存过
 * （交还配置后第一次保存），后端 400 会把这一格点名，页面照 `note bad` 接住。
 */
export function validateNotifyDraft(d: NotifyDraft): string | null {
  if (d.channel === 'bluebubbles') {
    if (!isUsableEndpoint(d.bluebubblesUrl.trim())) {
      return 'BlueBubbles 端点要写成 http:// 或 https:// 开头的完整地址（如 http://127.0.0.1:1234）。';
    }
    if (!d.bluebubblesPassword.trim()) {
      return '请填写 BlueBubbles 的 password。';
    }
    if (!d.bluebubblesRecipient.trim()) {
      return '请填写 iMessage 收件地址（Apple ID 或手机号）。';
    }
    return null;
  }
  if (!isUsableEndpoint(d.webhookUrl.trim())) {
    return 'webhook 地址要写成 http:// 或 https:// 开头的完整 URL（含 token）。';
  }
  return null;
}

/**
 * 保存（PUT /notify/channel）规则：只带**当前通道**的件；秘密是掩码或留空 → **省略**，
 * 后端按「不改」沿库里的值——绝不把掩码当值回传（`lib/providers.ts::buildProviderPatch`
 * 的同一姿态）。
 */
export function buildNotifyChannelPayload(d: NotifyDraft): NotifyChannelPayload {
  const payload: NotifyChannelPayload = { channel: d.channel };
  if (d.channel === 'bluebubbles') {
    payload.bluebubbles_url = d.bluebubblesUrl.trim();
    payload.bluebubbles_recipient = d.bluebubblesRecipient.trim();
    const pw = d.bluebubblesPassword.trim();
    if (pw && pw !== NOTIFY_SECRET_MASK) payload.bluebubbles_password = pw;
  } else {
    payload.webhook_url = d.webhookUrl.trim();
  }
  return payload;
}

/** 探针（POST /notify/test）请求体规则——与保存规则同源，掩码绝不回传。 */
export function buildNotifyTestPayload(d: NotifyDraft): {
  bluebubbles_url: string;
  bluebubbles_password?: string;
} {
  const payload: { bluebubbles_url: string; bluebubbles_password?: string } = {
    bluebubbles_url: d.bluebubblesUrl.trim(),
  };
  const pw = d.bluebubblesPassword.trim();
  if (pw && pw !== NOTIFY_SECRET_MASK) payload.bluebubbles_password = pw;
  return payload;
}
