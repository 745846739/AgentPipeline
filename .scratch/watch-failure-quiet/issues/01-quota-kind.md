# 01: 额度 / 账单类失败独立成 `llm_quota`

**What to build:** `LlmErrorKind` 加一档 `Quota`（稳定标识 `llm_quota`），让「余额 / 配额不足」
不再落进「网络」那一档。

**为什么**：2026-09-24 实测，本地代理回的是 `HTTP 400 …insufficient credits…`，而 `from_http`
认不出这段错误体 → `Error::Llm` → 台账类别读作 `llm_network`、指引写「请检查 base_url 是否正确、
网络是否可达」。一次账单问题被读成配置问题（指引错），且按网络类的节奏重试（票 02 的分档靠它分家）。

判据（`from_http`）：

- `402` → `Quota`（Payment Required 就是这个意思）。
- `400` 且错误体认出额度 / 账单字样 → `Quota`：`insufficient credit`、`insufficient_quota`、
  `quota exceeded`、`payment required`、`billing`、`balance`。
- 其余一档不动：`401 / 403` 仍是 `Auth`，`404` 仍是 `ModelNotFound`，400 的模型名 / 超长两支照旧。

**Blocked by:** —

**Status:** done

- [x] `LlmErrorKind` 加 `Quota` + `as_str()` = `llm_quota` + `advice()`（人话：这不是网络问题）
- [x] `from_http` 加 `402` 与 400 的额度字样识别（**放在模型名 / 超长两支之后**，次序即优先级）
- [x] `error.rs` 的类别注释补 `llm_quota`
- [x] 单测：402 → Quota；`insufficient credits` 的 400 → Quota；既有四档逐条不回归
- [x] 单测：一句**只含 model 字样**的 400 仍判 `ModelNotFound`（新规则不吃掉老规则）

**实施收尾（2026-09-25）:**

- **关键词表多认了几种同义写法**（`insufficient quota` / `exceeded your quota` /
  `payment required` / `billing` / `insufficient balance`）：「余额不足」在各家代理与供应商那里
  的措辞不统一，而这一档判错只会把指引引向网络。**次序仍在模型名与超长之后**——那两支更具体，
  且额度体里不会同时出现模型名或长度字样（有专门一条断言钉住「新规则不吃老规则」）。
- **advice 一句话说清两件事**：不是网络问题 + 动作在 provider 那一侧（续费 / 换 provider）。
- `error.rs` 的稳定标识注释同步；`docs/agents.md` 无该枚举的清单，无需改。
