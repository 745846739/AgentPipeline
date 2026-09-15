# 10: 远程 registry 安装

**What to build:** 从远程 registry 搜索并安装技能：索引格式须定义（技能名、版本、摘要、来源、
描述）；下载后按 `sha256` 校验，不符即拒绝；**来源白名单**默认只放行配置内的源。

这是本 effort **唯一新增的接缝**：市场客户端 trait（返回 `(bytes, sha256, source)`），测试用 fake
提供固定字节与摘要，**不打真网络**。

**Blocked by:** 09（技能导入——复用其落盘、同名冲突、结构校验）

**Status:** done

- [x] 定义索引格式与获取路径；索引条目含技能名、版本、`sha256`、来源、描述
- [x] 搜索：按关键词查 registry 并返回候选清单
- [x] 安装：下载 → `sha256` 校验 → 不符**拒绝安装**并报出期望值与实际值
- [x] **来源白名单**：默认只允许配置内放行的源（照 Claude Code `strictKnownMarketplaces` /
      Codex `allowed_sources` 的姿态）；未放行的源拒绝安装
- [x] 网络失败给出可归因报错（超时 / DNS / HTTP 状态），不与「摘要不符」「来源未放行」混为一谈
- [x] 市场客户端 trait 是唯一接缝；测试用 fake 覆盖：正常安装 / 摘要不符 / 来源未放行 / 索引畸形 /
      网络失败五条路径，**全部不打真网络**
- [x] 本机无网时票 09 的功能不受影响（网络失败不得阻塞本地导入）

**Notes（实现提示）:**
- **明确不做**（写进实现结论）：签名与人工审核队列。本批只做摘要 + 白名单 + 装前预览。
  摘要校验只能证明「没被改过」，证明不了「内容是善意的」——后者由票 11 的预览与信任标记承担。
- 传输层复用既有 HTTP 抽象，不引入第二个 HTTP 客户端栈。

---

## 实现结论

### 索引格式（本票定义）

`GET {source}/index.json`：

```json
{
  "skills": [
    {
      "name": "grill-me",
      "version": "1.2.0",
      "sha256": "9f86d0818…",
      "source": "https://skills.example.com",
      "description": "拷问设计树",
      "url": "https://skills.example.com/skills/grill-me-1.2.0.zip"
    }
  ]
}
```

六个字段（票面要求的前五个 + `url`）。`url` 单独给是因为**包与索引允许不同源**（CDN 常见）——
正因如此，安装时**下载地址的 origin 也须在白名单内**，否则白名单会被一个指向别处的索引条目绕过。

**每条的缺失 / 非法字段一律 fail fast**（`market_index_malformed`），不静默跳过该条：一条缺
`sha256` 的条目若被跳过，用户看到的是「搜索不到我要的技能」而不是「这个 registry 的索引坏了」，
排查方向完全错。摘要在**下载之前**就校验格式（64 位十六进制）——早报错好过让用户下完几十兆才失败。

### 五类失败互不混淆

| `kind` | HTTP | 用户该做什么 |
|---|---|---|
| `market_network` | 502 | 检查网络 / 稍后重试（下游不可达，不是请求错） |
| `market_not_found` | 404 | 换个技能名（索引里就没这条） |
| `market_digest_mismatch` | 400 | 怀疑中间人或索引过期，与来源方核对 |
| `market_source_not_allowed` | 400 | 改 `[market] allowed_sources` |
| `market_index_malformed` | 400 | 找 registry 维护者 |

几种动作毫无交集，混成一个「市场错误」等于没报错。姿态沿用 `Error::LlmClassified`：稳定 `kind` +
中文可操作 `message`（面向用户）+ `raw`（原始诊断）。`raw` 经 `ApiError` 的 `detail` 字段下发到响应体
（前端只读 `error`，故不破坏既有契约），用户截屏报障时不必再翻日志。

### 摘要的权威值是现算的，不采信传输层

`verify_digest` 对 `bytes` 现算 sha256，与索引钉住的值比对；**不采信 `Downloaded.sha256`**。若采信，
一个被控制的客户端可以同时改内容与声称值，校验形同虚设。传输层声明值与现算值不一致时**也**报错
（额外的篡改信号，零成本）。

> **这条最初没有被钉住。** 第一次牙齿检查把 `actual` 改成 `downloaded.sha256.trim().to_lowercase()`
> 后**所有摘要用例仍然全绿**——因为既有 fake 总是如实上报摘要，「传输层撒谎」这条攻击面根本没被构造。
> 补 `FakeMarket::serving_with_lying_digest`（索引钉诚实摘要、传输送换过的字节并谎称摘要相符）后
> 再跑牙齿检查，该用例如期失败（`Ok(PackageInfo)` —— 换过内容的包被装上了）。见
> `crates/core/tests/market.rs::transport_digest_is_never_trusted_over_the_computed_one`。

### 四项安全边界（code-review 两轴各自独立指出，均已修 + 钉）

评审的两个轴**独立**命中了同一个最严重的缺口，值得单独记：

**① 重定向绕过白名单（真实漏洞）。** reqwest 缺省 `Policy::limited(10)`，会跟最多 10 跳**跨源**
跳转，而白名单判定看的是**请求** URL——一个**已放行**的来源只要回一个 302，就能把内容指到任意
别处（内网元数据端点之类），白名单当场失效。两层修：生产客户端显式 `Policy::none()`（请求即最终
地址）；core 侧另加一道**独立**判定，复检 `Downloaded.source`（字节的**实际**来源）。后者的牙齿
检查：停用该判定后 `redirected_origin_is_rejected_even_when_the_requested_url_is_allowed` 如期
失败。

**② 明文 http 下摘要挡不住中间人。** `normalize_origin` 允许 `http`，而 `sha256` 在明文传输上
毫无意义——攻击者可同时替换索引与包，使摘要自洽通过。已收紧为**非回环来源必须 https**（回环
放行 `http`，便于本机起 registry 开发），在 `Config::validate` 解析期 fail fast。

**③ 下载体积无上限（远程比本地更宽的路）。** 本地导入受 `DefaultBodyLimit` 64 MiB 约束，远程
下载却 `response.bytes()` 一次读完、无任何上限——与票面「远程包不比本地上传的包享有更宽的路」
的意图相反。已加 `MAX_DOWNLOAD_BYTES = 64 MiB`（同值）：声明式 `Content-Length` 早退 + 流式
累加兜底。

**④ `raw` 写而不读。** `Error::Market` 的 `raw`（期望/实际摘要、HTTP 状态、索引片段）此前无处
可达——`Display` 只有 `message`，API 层也丢弃它。已在 `ApiError` 加 `detail` 字段并作为响应体的
额外字段下发，与面向用户的 `error` 分开。

### 摘要 ≠ 安全：更正一处过度声称

初稿的「实现结论」写了「这条边界写进了模块头注释、`docs/agents.md` **与错误提示**」——**错误提示
里其实没有**，是该次评审抓出来的不实声称。已改：边界落在模块头与 `docs/agents.md`，错误提示只报
事实与动作（不复述这段定位说明）。

### 安装顺序：每一步都尽量在下载之前失败

0. **白名单为空即拒**（早于拉索引——使「默认拒绝」是本函数的结构性质，而非只靠上层记得别建客户端）
1. 索引里找到条目（没找到 → 404，不下任何东西）
2. 条目的 `source` 放行？
3. 条目的**名字**可用作目录名？（复用票 09 的 `check_skill_name`，避免为一个注定落不了盘的名字白烧下载）
4. **下载地址的 origin** 也放行？（跨源场景，见上）
5. 下载，并复检字节的**实际来源** origin（挡重定向，见下）
6. 摘要校验
7. 交给票 09 的 `skill_import::install` 落盘

第 6 步**复用票 09 的同一入口**（`SkillPackage::from_zip` + `install`），因此结构校验、同名冲突、
路径穿越（zip 条目名两道独立判定 + realpath 实数校验）一处生效、两处受益。**远程包不比本地
上传的包享有更宽的路**——有一条用例专门钉这一点（穿越包即使摘要对得上也被拒）。

### 唯一新增接缝

`MarketClient` trait（`index` / `download`）是本 effort 唯一新增的可测试性接缝（决策 143）。
生产实现 `HttpMarketClient`（reqwest，复用既有 HTTP 栈，未引入第二套客户端）；测试实现
`crates/testkit/src/market_fixture.rs` 的 `FakeMarket`。

**fake 放在 testkit 而非 core 单测里**，因为它被**两层**使用：core 的 L2 集成测试驱动
`install_from_market`，L3 契约测试把它注入 `AppState` 驱动端点。这也带来一个约束：core 的内联
`#[cfg(test)] mod tests` **用不了** testkit 的 fake（dev-dependency 链了另一份 core，类型不互通，
与 `FakeAgent` 同）。因此分层是：

- **纯函数**（索引解析 / 搜索 / 白名单 / 摘要 / `origin_of`）→ 放在 `src/agent/market.rs` 内联单测；
- **需要 fake 的端到端**（安装落盘 / 五条失败路径 / 穿越 / 同名）→ 放 `crates/core/tests/market.rs`；
- **端点契约** → `crates/app/tests/api_contract.rs`，经 `AppState::with_market` 注入。

### 空白名单是合法状态，不是错误

`AppState.market: Option<Arc<dyn MarketClient>>`——白名单为空时生产不构造客户端（`None`），端点
返回 400 并说明「怎么开」（在 `[market] allowed_sources` 里加来源）以及「本地导入不受影响」。
不是 500：空白名单是**有意支持的配置**（= 只装本地技能），不是服务器故障。

### 明确不做（写进文档）

签名（Sigstore 式）与人工审核队列。本批只有摘要 + 白名单。**摘要只能证明「没被改过」，证明不了
「内容是善意的」**——这条边界写进了模块头注释、`docs/agents.md` 与错误提示，因为不写清用户会
误以为摘要=安全。善意性由票 11 的装前预览与信任标记承担。

### 代码审视发现并修掉的问题

1. `market_not_found` 最初落进 400 兜底（契约用例断言 404 时暴露）。它该是 404——用户该做的动作
   是换个名字，与「摘要不符」这种要改配置的情形不同。已按 kind 分开映射。
2. `map_market_error` 最初用 `&err` + `err.to_string()`，但 `Error` 不是 `Clone`；改成按值匹配后
   `MessageOrRaw` 那个多余助手也随之删掉（`Display` 就是 `message`）。
3. **搜索只查了 `source`，没查下载地址的 origin**：`source` 放行但 `url` 指向别处的条目会**出现
   在候选里**，用户点了才发现装不上——而候选侧存在的意义正是「看不到装不上的东西」。已与安装侧
   对齐（两处都查），并补搜索侧用例与契约用例。
4. **`Downloaded.source` 写而不读**（接缝文档声称它是「白名单判定输入」，实际生产路径从不读）。
   已把它变成真正的判定点（见上面 ① 的第二层）。
5. `HttpMarketClient::download` 的注释写「传输层若给了 `X-Checksum-Sha256` 就带上」，代码却始终
   填现算值——文档与实现相反。已改为如实描述（本实现不从传输层取任何声明值）。
6. `Error::Market` 的 `kind` 取值此前要在测试里手写两遍 `match`。已加 `Error::market_kind()`
   访问器（与既有的 `llm_classified()` 并列），测试助手与 API 映射共用。
7. 文档准确性：`docs/testing.md` 的用例数（19→18 / 9→12 / 4→5 / 11→14）、`testkit/market_fixture.rs`
   头部把「core 单测」误写成使用方（实为 L2 集成测试）、`market.rs` 模块头「四类失败」实为五类。
8. **接缝编号冲突**：`docs/implementation.md` 新增「第五条接缝」时，`docs/testing.md` §3.1 的
   权威表已把主题契约（决策 169）称为第 5 条。已把 `MarketClient` 补进权威表称第 5 条，主题契约
   改称第 6 条，两处口径统一。
9. 决策登记：决策 143 被修订（四条接缝 → 五条）此前只在正文内联提及，没有独立决策号，只读
   `decisions.md` 的 agent 会以为新增接缝违规。已追 **决策 177**（176 被并行会话的对讲台迁移文件
   占用），并在 143 行显式标注修订关系。

### 遗留

- **`HttpMarketClient` 无单测覆盖**（它只是 reqwest 的薄壳）。评审据此指出两处未测的真实行为：
  ① `network_error` 里 `is_timeout` / `is_connect` / `is_decode` 的分支，以及两个 `!status.is_success()`
  分支——即票面第 5 项「可归因」的**粒度**本身没被测（现有用例只断言 `kind` 与「网络」字样，而
  fake 的报文是测试自己写的）；② 上面 ① 的重定向行为（`Policy::none()` 的效果）没有测试能覆盖
  ——`FakeMarket` 结构上就模拟不了重定向。两条都需要一个本地 HTTP 服务才能测，故与真 LLM 冒烟
  同姿态（不入常规闸门）。**这是本票最实在的未覆盖面**，票 12（出口控制）或后续可一并处理。
- DNS 解析失败与连接被拒目前折叠成同一条文案（「DNS 解析失败 / 连接被拒 / TLS 握手失败」），
  票面提到的「DNS」并未**单独**可归因——reqwest 不区分这两者，除非另做解析试探。
- **签名与人工审核队列**不在本批（见票面 Notes）；装前预览与信任标记是票 11。
