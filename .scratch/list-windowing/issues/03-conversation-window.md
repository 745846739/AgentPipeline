# 03: 会话页签接线——run 药丸搜索 + 消息切片（显尾部）

**What to build:** `frontend/src/components/task/ConversationViewer.svelte`：
run 药丸行（:70）加关键词过滤（run id / 节点 / 状态）；选中 run 的消息列表（:103）
接切片原语（默认尾部），整段一次性渲染变窗口化。移动版药丸横滚既有行为不动。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 药丸过滤框（placeholder 讲清它滤的是 run 行）
- [x] 消息列表切片显尾部 + 展开提示；MessageBubble / ToolCallCard 渲染不变
- [x] 单测：过滤纯函数；长会话（500+ 消息 mock）首屏只渲 50 条
