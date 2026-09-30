/// 失败回合在台账里的标记（票 04）。对讲台按它把这一轮渲染成失败轮，不是一个中性轮。
pub const FOREMAN_FAILED_TURN_MARK: &str = "【没跑起来】";

// ───────────────── 播报的归因类别（决策 227 / 235 / 238）─────────────────

/// 归因结构块的哨兵（决策 238）：**回话文本里约定的一段**，后端解析。
///
/// 为什么走回话文本而不是新工具：仓里已经靠回话里的哨兵 / 前缀传机器可读信号
/// （`【无需处理】` 模型发后端认、[`FOREMAN_WATCH_MARK`] 后端加前端认），而归因类别
/// **每一轮播报都要带**——做成工具调用等于每轮多一次模型往返，而实测里一轮的往返成本
/// 已经在十万 token 量级（决策 238 的两条理由）。
///
/// 为什么不是 `traces_json`（决策 235 明确否决）：那是**后端自己写的**观测数据，装不了
/// 「模型的归因声明」——而那正是要被校验的东西，让产者与证者同一人等于没校验。
pub const FOREMAN_ATTRIBUTION_MARK: &str = "【归因】";

/// 归因类别（决策 227 的四类）。**「我不知道是什么问题」不是结论**，故四类之外不许收口。
///
/// 四类的证据面、修法、授权都不一样，用一个词盖住就会在播报里丢掉「该找谁」这个信息——
/// 而值班长的全部价值就在这个信息上（决策 227 的起因：一次实测里三条故障各自属于不同类别）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionKind {
    /// 宿主环境：未签名的壳、系统调用被拦、磁盘 / 权限一类。
    Host,
    /// 流水线运行：节点自身跑挂、重试耗尽、循环 / 调度。
    Pipeline,
    /// 目标项目代码：只有这一类才吃 `repair` 那条 worktree 链。
    ProjectCode,
    /// prompt 与配置：provider / persona / 阶段配置 / 工具声明。
    PromptConfig,
}

impl AttributionKind {
    pub const ALL: [AttributionKind; 4] = [
        AttributionKind::Host,
        AttributionKind::Pipeline,
        AttributionKind::ProjectCode,
        AttributionKind::PromptConfig,
    ];

    /// 稳定标识（落库 / 线上 / 界面都按它判，不按文案）。
    pub fn as_str(self) -> &'static str {
        match self {
            AttributionKind::Host => "host",
            AttributionKind::Pipeline => "pipeline",
            AttributionKind::ProjectCode => "project_code",
            AttributionKind::PromptConfig => "prompt_config",
        }
    }

    /// 给人看的词（界面标记与播报里的那一类）。
    pub fn label(self) -> &'static str {
        match self {
            AttributionKind::Host => "宿主环境",
            AttributionKind::Pipeline => "流水线运行",
            AttributionKind::ProjectCode => "目标项目代码",
            AttributionKind::PromptConfig => "prompt 与配置",
        }
    }

    /// 稳定标识或中文词 → 类别。两者都认是**刻意的**：模型写出中文词是一件正常事，
    /// 不认它只会多出一条「格式不合规」的假未定位。产物一律是稳定标识（[`Self::as_str`]）。
    pub fn parse(raw: &str) -> Option<Self> {
        let value = raw.trim();
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == value || k.label() == value)
    }
}

/// 一次回话里那个结构块的解析结果。
///
/// **「根本没给」与「四类之外」由这一个解析点判出**（决策 238）：两条都算**未定位**，
/// 因为对判据（决策 230 的第四项）而言它们是同一件事——这一次收口没有可校验的归因类别。
/// 分开记的是原因，不是结论。
///
/// **`run_id` 是判据①的校验面**（决策 230 的「证据归错 run 与没有证据同判失败」）：
/// 类别单独一个字段装不下「这次说的是哪条 run」，而 09-19 翻车的正是这一件——回话读起来
/// 毫无破绽、类别也合规，只是把 run 27 的活栈记在了 run 26 名下，而**没有任何断言拦得住**。
/// 故结构块同时带 `run_id`：它让「回话说的是哪条 run」与「证据说的是哪条 run」可比。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// 定位成功：给出了四类之一。
    Located {
        kind: AttributionKind,
        /// 回话自己指名的 run（判据①）。**可以为空**：不是每条播报都针对某一条 run
        /// （「无需处理」、或一次说的是任务级态势），而那种情况下「没指名」是诚实的，
        /// 不该被逼着编一个数。校验交给读它的那一方（诊断包的 `latest_attribution`
        /// 与总闸用例），这里只如实带出。
        run_id: Option<i64>,
    },
    /// 没给结构块（回话里一行哨兵都没有）。
    Missing,
    /// 给了但不可用：四类之外 / 缺 `attribution` / JSON 坏了 / 多处自相矛盾。
    Invalid { payload: String, why: &'static str },
}

impl Attribution {
    pub fn kind(&self) -> Option<AttributionKind> {
        match self {
            Attribution::Located { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// 回话明确指名的 run（判据①）。未定位或没指名时是 `None`。
    pub fn run_id(&self) -> Option<i64> {
        match self {
            Attribution::Located { run_id, .. } => *run_id,
            _ => None,
        }
    }

    /// 这一次收口算不算「定位成功」的那一项（决策 230 判据④）。
    pub fn is_located(&self) -> bool {
        matches!(self, Attribution::Located { .. })
    }

    /// 线上 / 界面用的串：四类之一，或 `unlocated`（**不编一个假的类别**）。
    pub fn wire(&self) -> &'static str {
        self.kind().map(|k| k.as_str()).unwrap_or("unlocated")
    }

    /// 未定位时的原因串（定位成功时为 `None`）——给排查看，不给界面当类别使。
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Attribution::Located { .. } => None,
            Attribution::Missing => Some("missing"),
            Attribution::Invalid { why, .. } => Some(why),
        }
    }
}

/// 结构块的载荷上限：超出的部分不留在结果里（它是模型写歪的一段文本，不是证据）。
const ATTRIBUTION_PAYLOAD_MAX_CHARS: usize = 200;

/// 从回话文本里解析归因类别（决策 235 的载体 + 决策 238 的发射方式）。
///
/// 三条判据：
/// * **结构块以整行出现**（行首哨兵，载荷在同一行）才算块——行文里提一句哨兵是散文，
///   不参与判定。于是「夹带」只剩一种形态：再写一行块，而那一行照样要过下面的判据。
/// * **按最严的一处判**：一条回话里有多个块时，任何一处不合规都不收口（决策 235 的
///   「四类之外不许收口」）；多处互相矛盾同样不收口——自相矛盾不是结论。
/// * 找不到块 = `Missing`；**它与「四类之外」在判据上是同一件事**（都没给出可校验的
///   类别），分开记的只是原因（决策 238）。
///
/// 这个方向是刻意的：宁可判未定位，也不让一段夹带的一个合规字样把整轮说成已定位。
pub fn parse_attribution(text: &str) -> Attribution {
    let payloads: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix(FOREMAN_ATTRIBUTION_MARK)
                .map(|rest| rest.trim())
        })
        .collect();
    if payloads.is_empty() {
        return Attribution::Missing;
    }
    // 类别与它自己指名的 run 同批记：**两处「类别一致但 run 不同」是矛盾**（一处说
    // run 26、一处说 run 27），而只比类别会把这种形状当成「复述同一件事」放过去——
    // 那正是 09-19 那次回话的形状（决策 230 判据①）。
    let mut located: Option<(AttributionKind, Option<i64>)> = None;
    for payload in payloads {
        let truncated =
            || crate::storage::observability::truncate_text(payload, ATTRIBUTION_PAYLOAD_MAX_CHARS);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            return Attribution::Invalid {
                payload: truncated(),
                why: "JSON 解析失败",
            };
        };
        let Some(raw) = value.get("attribution").and_then(|v| v.as_str()) else {
            return Attribution::Invalid {
                payload: truncated(),
                why: "缺 attribution 字段",
            };
        };
        let Some(kind) = AttributionKind::parse(raw) else {
            return Attribution::Invalid {
                payload: truncated(),
                why: "四类之外",
            };
        };
        // 判据① 的校验面：`run_id` 与类别同批给。**它不是必填**（有的播报说的是任务级态势，
        // 指不出单条 run），但一旦给了就必须是正整数——给个字符串或 0 会把「按 run 对账」
        // 这件事悄悄变成一个假读数（0 在库里同样是锚点值，与决策 231 的哨兵归一同一姿态）。
        let run_id = match value.get("run_id") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => match v.as_i64() {
                Some(id) if id > 0 => Some(id),
                _ => {
                    return Attribution::Invalid {
                        payload: truncated(),
                        why: "run_id 不是正整数",
                    }
                }
            },
        };
        match located {
            None => located = Some((kind, run_id)),
            Some((previous, previous_run)) if previous == kind && previous_run == run_id => {}
            // 类别一致但 run 不同：**那不是复述，是两处互相矛盾**——正是要拦的形状
            // （一处说 run 26、一处说 run 27，两处都「合规」，合起来是错的）。
            Some(_) => {
                return Attribution::Invalid {
                    payload: truncated(),
                    why: "多处自相矛盾",
                }
            }
        }
    }
    match located {
        Some((kind, run_id)) => Attribution::Located { kind, run_id },
        None => Attribution::Missing,
    }
}

/// 播报里那段「必填」的规格（决策 227 的必填 + 235 的载体 + 238 的形态）。
///
/// 写进 system prompt 而不是人格文本：人格是可被 `persona_path` 覆盖的（决策 7），
/// 而这条要求**没有校验点就不能少**——它是决策 230 判据④的载体，覆盖人格的人不该
/// 顺手把校验面一起覆盖掉。四类的词与稳定标识都由 [`AttributionKind`] 生成（同源）。
pub(super) fn attribution_discipline() -> String {
    let choices = AttributionKind::ALL
        .iter()
        .map(|k| format!("{}（{}）", k.as_str(), k.label()))
        .collect::<Vec<_>>()
        .join(" / ");
    format!(
        "\n## 播报的归因（必填）\n\
         每一次**播报**（值守轮，以及你回答「哪里出了什么问题」时）末尾都要单独一行给出归因类别，\n\
         机器可读，照这个形状写：\n\
         {FOREMAN_ATTRIBUTION_MARK}{{\"attribution\":\"host\",\"run_id\":123}}\n\
         取值只能是这四类之一：{choices}。\n\
         四类之外不许收口：写别的词等于没给。四类的证据面、修法、授权各不相同——\n\
         「我不知道是什么问题」不是结论，宁可用一条证据把范围收到最像的那一类并说清还缺什么。\n\
         说某一条 run 时**同时带上它的 id**（`run_id`，正整数，取台账里的那一行）：\n\
         你采到的证据是**哪一条 run 的**，写在别处没人对得了账——报错 run 与没有 run 同判未定位。\n\
         说的若是任务级态势、指不出单条 run，就**不写** `run_id`（不写是诚实的，编一个数不是）。\n"
    )
}
