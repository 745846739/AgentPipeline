import { describe, expect, it } from 'vitest';

import {
  CHANNEL_LABELS,
  NOTIFY_SECRET_MASK,
  buildNotifyChannelPayload,
  buildNotifyTestPayload,
  draftFromSettings,
  isUsableEndpoint,
  notifyOriginLabel,
  validateNotifyDraft,
  type NotifyDraft,
} from './notifyChannel';
import type { NotifySettings } from '../api/types';

/**
 * 「离线通知」设置页判据的单测（决策 272⑥⑦⑧）。
 *
 * 钉的是**按下按钮之前**就能看见的那一层：通道四件的整体语义（只送当前通道的件）、
 * 秘密的掩码纪律（掩码绝不回传）、端点形状与必填件、两级来源的中文标签
 * （照 `sharePairing.ts` 的口径）。后端是权威——够不够得着由 ping 说，这里不测。
 */

function draft(over: Partial<NotifyDraft> = {}): NotifyDraft {
  return {
    channel: 'bluebubbles',
    webhookUrl: '',
    bluebubblesUrl: 'http://127.0.0.1:1234',
    bluebubblesPassword: 'pw',
    bluebubblesRecipient: 'me@icloud.com',
    ...over,
  };
}

describe('notifyChannel（决策 272）', () => {
  it('掩码纪律：password 是掩码时，保存与探针载荷都省略该字段，绝不回传掩码', () => {
    const d = draft({ bluebubblesPassword: NOTIFY_SECRET_MASK });
    const payload = buildNotifyChannelPayload(d);
    expect(payload.channel).toBe('bluebubbles');
    expect(payload.bluebubbles_password).toBeUndefined();
    expect(JSON.stringify(payload)).not.toContain(NOTIFY_SECRET_MASK);
    const test = buildNotifyTestPayload(d);
    expect(test.bluebubbles_password).toBeUndefined();
  });

  it('保存载荷只带当前通道的件：generic/feishu 只送 webhook_url，bluebubbles 只送三件', () => {
    const bb = buildNotifyChannelPayload(draft());
    expect(bb.webhook_url).toBeUndefined();
    expect(bb.bluebubbles_url).toBe('http://127.0.0.1:1234');
    expect(bb.bluebubbles_password).toBe('pw');
    expect(bb.bluebubbles_recipient).toBe('me@icloud.com');

    const generic = buildNotifyChannelPayload(
      draft({ channel: 'feishu', webhookUrl: 'https://open.feishu.cn/hook/x' }),
    );
    expect(generic.webhook_url).toBe('https://open.feishu.cn/hook/x');
    expect(generic.bluebubbles_url).toBeUndefined();
    expect(generic.bluebubbles_password).toBeUndefined();
    expect(generic.bluebubbles_recipient).toBeUndefined();
  });

  it('校验：缺必填件逐项报错；端点必须带 scheme 与主机', () => {
    expect(validateNotifyDraft(draft())).toBeNull();
    expect(validateNotifyDraft(draft({ bluebubblesUrl: '127.0.0.1:1234' }))).toMatch(/http/);
    expect(validateNotifyDraft(draft({ bluebubblesPassword: ' ' }))).toMatch(/password/);
    expect(validateNotifyDraft(draft({ bluebubblesRecipient: '' }))).toMatch(/收件地址/);
    expect(
      validateNotifyDraft(draft({ channel: 'generic', webhookUrl: '' })),
    ).toMatch(/webhook/);
    expect(
      validateNotifyDraft(draft({ channel: 'generic', webhookUrl: 'not a url' })),
    ).toMatch(/http/);
  });

  it('掩码是合法的已存值：校验通过（真正没有存过时由后端 400 点名）', () => {
    const d = draft({ bluebubblesPassword: NOTIFY_SECRET_MASK });
    expect(validateNotifyDraft(d)).toBeNull();
  });

  it('端点形状的尺与 providers 的 isUsableBaseUrl 同一口径', () => {
    expect(isUsableEndpoint('http://127.0.0.1:1234')).toBe(true);
    expect(isUsableEndpoint('https://bb.local')).toBe(true);
    expect(isUsableEndpoint('ftp://x')).toBe(false);
    expect(isUsableEndpoint('not a url')).toBe(false);
  });

  it('读数 → 草稿：秘密以回显值预填；没有通道声明时落到 generic', () => {
    const s: NotifySettings = {
      enabled: true,
      channel: 'bluebubbles',
      origin: 'settings',
      webhook_url: '',
      bluebubbles_url: 'http://127.0.0.1:1234',
      bluebubbles_password: NOTIFY_SECRET_MASK,
      bluebubbles_recipient: 'me@icloud.com',
      cooldown_sec: 300,
      quiet_hours: [22, 8],
      politeness_origin: 'config',
    };
    const d = draftFromSettings(s);
    expect(d.channel).toBe('bluebubbles');
    expect(d.bluebubblesPassword).toBe(NOTIFY_SECRET_MASK);
    expect(draftFromSettings({ ...s, channel: null }).channel).toBe('generic');
  });

  it('来源标签照 sharePairing 的口径：界面上的选择 / 配置文件', () => {
    expect(notifyOriginLabel('settings')).toBe('界面上的选择');
    expect(notifyOriginLabel('config')).toBe('配置文件');
    expect(notifyOriginLabel(null)).toBe('配置文件');
  });

  it('三个通道的界面名一个不少（值域与后端 NotifyFormat 的 serde 小写形一致）', () => {
    expect(Object.keys(CHANNEL_LABELS).sort()).toEqual(['bluebubbles', 'feishu', 'generic']);
  });
});
