# AI Tool Calling — Implementation Plan

> 实现目标：让 AI 角色能通过工具调用读取文件、执行系统查询命令，并经过用户安全审批。
> 参考架构文档：`docs/architecture/tool-calling-arch.md`

---

## 0. 实施前检查

- [ ] 确认 `AiStreamEvent` 的 `Clone` 派生（需要新增 `ToolCall` 变体）
- [ ] 确认 `ChatMessage` 支持 `role: "tool"` + `tool_call_id` 字段
- [ ] 确认当前 `reqwest::blocking` client 支持非 streaming 请求（用于 tool_call 回传续轮）

---

## Phase 0: 数据结构层

### T0.1 扩展 ChatRole / ChatMessage (`ai/types.rs`)

**改动**：

```diff
 pub enum ChatRole {
     User,
     Assistant,
     System,
+    Tool,
 }

 pub struct ChatMessage {
     pub role: ChatRole,
     pub content: String,
     pub timestamp: f64,
+    /// 仅 role == Tool 时使用
+    pub tool_call_id: Option<String>,
+    /// 仅 role == Assistant 且 LLM 发起 tool_call 时使用
+    pub tool_calls: Option<Vec<ToolCall>>,
 }
```

**新增类型**：

```rust
/// LLM 发起的工具调用指令
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// JSON string arguments
    pub arguments: String,
}

/// 工具定义声明，随 API 请求发送
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub r#type: String,  // always "function"
    pub function: ToolFunctionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// AI 状态机
#[derive(Debug, Clone, PartialEq)]
pub enum AiState {
    Idle,
    Waiting,
    PendingTool {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    Executing,
}
```

**测试**：
- [ ] `ChatMessage` JSON 序列化/反序列化，确认 `tool` role 的格式
- [ ] `ToolCall` 结构体的字段解析

### T0.2 扩展 AiConfig (`ai/types.rs`)

```diff
 pub struct AiConfig {
     // ... 现有字段 ...
+
+    // ── Tool Calling ──
+    pub tool_calling_enabled: bool,
+    pub max_tool_rounds: u32,
+    pub allowed_commands: Vec<String>,
+    pub allowed_read_paths: Vec<String>,
 }
```

**默认值**：

```rust
tool_calling_enabled: false,       // 默认关闭，用户主动开启
max_tool_rounds: 10,
allowed_commands: vec![],          // 空=所有命令需审批
allowed_read_paths: vec![],
```

### T0.3 扩展 AiStreamEvent (`ai/types.rs`)

```rust
pub enum AiStreamEvent {
    Token(String),
    /// 一个完整的 tool_call 已经到达（delta 拼接完成）
    ToolCall(ToolCall),
    Done,
    Error(String),
}
```

---

## Phase 1: 工具引擎

### T1.1 创建 `ai/tools/mod.rs`

```rust
pub mod registry;
pub mod executors;
pub mod safety;
```

### T1.2 工具注册表 (`ai/tools/registry.rs`)

```rust
pub struct ToolRegistry {
    tools: HashMap<&'static str, ToolDefinition>,
    executors: HashMap<&'static str, fn(Value) -> Result<String, String>>,
    safety: HashMap<&'static str, SafetyLevel>,
}

impl ToolRegistry {
    pub fn builtin() -> Self;
    pub fn get_definitions(&self) -> Vec<ToolDefinition>;
    pub fn execute(&self, name: &str, args: Value) -> Result<String, ToolError>;
    pub fn safety_level(&self, name: &str) -> Option<SafetyLevel>;
}
```

**初始工具清单**：

| 工具名 | 功能 | 安全等级 | 参数 |
|---|---|---|---|
| `read_file` | 读取文本文件 | `Dangerous` | `{path: string, max_lines?: number}` |
| `exec_cmd` | 执行 shell 命令 | `Dangerous` | `{cmd: string, timeout?: number}` |
| `list_dir` | 列出目录 | `Dangerous` | `{path: string, max_entries?: number}` |
| `read_proc` | 读 /proc 信息 | `Safe` | `{pid?: number, stat?: string}` |
| `get_env` | 获取环境变量 | `Safe` | `{name: string}` |

### T1.3 执行器 (`ai/tools/executors.rs`)

```rust
pub fn exec_cmd(args: Value) -> Result<String, String> { ... }
pub fn read_file(args: Value) -> Result<String, String> { ... }
pub fn list_dir(args: Value) -> Result<String, String> { ... }
pub fn read_proc(args: Value) -> Result<String, String> { ... }
pub fn get_env(args: Value) -> Result<String, String> { ... }
```

**实现细节**：

- `exec_cmd`: 使用 `std::process::Command`，设置 `timeout`（默认 5s），捕获 `stdout` + `stderr`
- `read_file`: 使用 `std::fs::read_to_string`，`max_lines` 截断
- `exec_cmd` 路径：通过 `SHELL` 环境变量或默认 `/bin/sh -c`
- 所有结果最多返回 **4096 字符**，超出截断并追加 `...(truncated)`
- 错误返回必须以字符串形式，不能 panic

**测试**：
- [ ] `exec_cmd("echo hello")` → `"hello\n"`
- [ ] `exec_cmd("sleep 10")` → timeout error
- [ ] `read_file("/etc/hostname")` → hostname string
- [ ] `read_file("/nonexistent")` → error
- [ ] 4096 字符截断验证

### T1.4 安全层 (`ai/tools/safety.rs`)

```rust
pub enum SafetyLevel {
    Safe,
    Dangerous { description: &'static str },
}

pub struct SafetyConfig {
    pub allowed_commands: Vec<String>,
    pub allowed_read_paths: Vec<String>,
    pub max_tool_rounds: u32,
}

/// 检查命令是否在白名单内
pub fn is_command_allowed(cmd: &str, allowed: &[String]) -> bool;

/// 路径消毒：拒绝 .. 、未展开的 ~、symlink 检测
pub fn sanitize_path(path: &str, allowed_roots: &[String]) -> Result<PathBuf, String>;

/// 截断结果
pub fn truncate_result(s: &str, max_chars: usize) -> String;
```

**安全规则**：

| 规则 | 实现 |
|---|---|
| 命令白名单 | `is_command_allowed` 匹配第一个 token |
| 路径消毒 | 展开 `~` → `dirs::home_dir()`，拒绝 `..`，拒绝绝对路径不在允许前缀内的 |
| 超时 | `Command::spawn` + `wait_with_output` 带 timeout |
| 结果截断 | 统一 4096 字符 |
| 轮次上限 | `max_tool_rounds` 硬限制 |

**测试**：
- [ ] `is_command_allowed("ps aux", &["ps", "ls"])` → `true`
- [ ] `is_command_allowed("rm -rf /", &["ps", "ls"])` → `false`
- [ ] `sanitize_path("../../etc/passwd", &[])` → `Err`
- [ ] `sanitize_path("/home/user/file.txt", &["/home/user"])` → `Ok`
- [ ] `truncate_result(&"a".repeat(5000), 4096)` → 4096 chars + `...(truncated)`

---

## Phase 2: API 客户端适配

### T2.1 streaming tool_calls delta 拼接 (`ai/client.rs`)

当前 `send_stream` 只解析 `delta.content`。需要新增：

```rust
// 在流式循环中增加 tool_calls delta 拼接
let mut tool_call_acc: Vec<(usize, ToolCallBuilder)> = Vec::new();

// 每次解析 delta.tool_calls:
for tc in tool_calls_array {
    let index = tc["index"].as_u64().unwrap_or(0) as usize;
    // 按 index 分组，拼接 arguments 字符串
}
```

**delta 拼接原理**：

```
SSE chunk 1: {delta: {tool_calls: [{index: 0, id: "call_1", function: {name: "exec_cmd", arguments: "{\"cmd\":"}}]}}
SSE chunk 2: {delta: {tool_calls: [{index: 0, function: {arguments: " \"ps aux\"}"}}]}}
                                       ↓ 拼接
最终: {id: "call_1", function: {name: "exec_cmd", arguments: '{"cmd": "ps aux"}'}}
```

完成后 `send_stream` 的 `tx` 除了发送 `Token`，也要发送 `ToolCall` 事件。

### T2.2 新增非 streaming 请求方法 (`ai/client.rs`)

tool_call 回传续轮时不需要 streaming（结果很短），新增 `send_with_tools()`：

```rust
pub fn send_with_tools(
    &self,
    messages: &[ChatMessage],
    config: &AiConfig,
    tools: &[ToolDefinition],
) -> Result<AiResponse, String>;

pub struct AiResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}
```

- `stream: false`
- `tools: [...]`
- `tool_choice: "auto"`
- 返回解析 `choices[0].message`

**测试**：
- [ ] mock API 返回 tool_call → 正确解析 `ToolCall` 结构
- [ ] mock API 返回文本 → `content: Some(...)`, `tool_calls: []`
- [ ] mock API 同时返回 text + tool_call（极少情况但需处理）
- [ ] 错误处理：网络错误、API error、JSON parse error

### T2.3 Multi-turn loop 客户端组装

新增函数（不在 client 结构体内，在 `ai/mod.rs` 或 `ai/client.rs`）：

```rust
/// 处理整个 multi-turn tool calling 循环
///
/// 返回最终的 assistant 文本回复。
pub fn run_tool_loop(
    messages: &[ChatMessage],
    config: &AiConfig,
    registry: &ToolRegistry,
    safety: &SafetyConfig,
    approval_sender: &mpsc::Sender<ApprovalRequest>,
    approval_receiver: &mpsc::Receiver<ApprovalResponse>,
) -> Result<String, String>;
```

**逻辑**：

```python
for round in 0..max_rounds:
    response = api.send(messages, tools)
    if response.content:
        return response.content
    for tc in response.tool_calls:
        await_approval(tc) if dangerous
        result = registry.execute(tc.name, tc.args)
        messages.push(role:tool, content=result)
return "Max rounds reached"
```

---

## Phase 3: 状态机集成

### T3.1 替换 AppState 中的 ai_pending (`app.rs`)

```diff
- pub ai_pending: bool,
+ pub ai_state: crate::ai::types::AiState,
```

所有引用 `self.ai_pending` 的地方改为 `self.ai_state != AiState::Idle` 或模式匹配。

**需要替换的位置**（从 grep 结果已知 4+ 处）：
- `send_ai_message()`: `self.ai_pending` → `self.ai_state == Idle`
- `poll_ai_result()`: `self.ai_pending = true/false`
- `chat_panel.rs` 中的 `!pending` 引用
- settings panel 中的禁用控制

### T3.2 重写 `send_ai_message()`

```rust
pub fn send_ai_message(&mut self) {
    // 1. 现有逻辑：提取 text, push user message, 构建 api_messages

    // 2. 如果 tool_calling_enabled，附加工具定义
    let tools = if self.ai_config.tool_calling_enabled {
        Some(self.tool_registry.get_definitions())
    } else {
        None
    };

    // 3. 启动后台线程
    //    - 需要传递 tool_registry, safety_config, approval 通道
    //    - 线程内运行 run_tool_loop
    //    - stream token 仍然通过 mpsc 发送
}
```

### T3.3 重写 `poll_ai_result()`

```rust
pub fn poll_ai_result(&mut self) {
    match &mut self.ai_state {
        AiState::Idle => return,
        AiState::Waiting { rx, .. } => {
            // 现有 streaming 接收逻辑 + ToolCall 事件处理
            // 收到 ToolCall → 检查安全等级
            //   Safe → auto execute, 发送回传, 继续 Waiting
            //   Dangerous → 切到 PendingTool, 弹出审批
        }
        AiState::PendingTool { .. } => {
            // 等待用户操作 chat_panel 中的审批按钮
        }
        AiState::Executing => {
            // 短同步操作，一般不会轮询到这里
        }
    }
}
```

---

## Phase 4: 审批 UI

### T4.1 审批弹窗组件 (`ai/chat_panel.rs`)

在 chat_panel 底部或 modal 区域新增：

```rust
// 在 chat_panel UI 渲染中
if let AiState::PendingTool { ref tool_name, ref args, .. } = app.ai_state {
    show_tool_approval_dialog(ui, app, tool_name, args);
}
```

**UI 布局**：

```
┌─────────────────────────────────┐
│ ⚠ AI 请求：exec_cmd            │
│                                 │
│ 命令: ps aux | grep -i game    │
│                                 │
│ [✓ 本次允许] [✗ 拒绝]          │
│ [✓ 记住本次会话]               │
└─────────────────────────────────┘
```

**交互**：
- 点击"允许" → `app.approve_tool()` → 执行 → 回传 → `Waiting`
- 点击"拒绝" → `app.reject_tool()` → 回传 "Operation rejected by user" → `Waiting`
- 窗口外点击不关闭（必须做出选择，或可通过特定按钮最小化）

### T4.2 工具审批数据流

```rust
impl AppState {
    pub fn approve_tool(&mut self) {
        if let AiState::PendingTool { tool_call_id, tool_name, args } = self.ai_state.clone() {
            // 执行
            let result = self.tool_registry.execute(&tool_name, args);
            // 构造 role:tool 消息
            let msg = ChatMessage { role: Tool, content: result, tool_call_id: Some(tool_call_id), ... };
            // 续请求（后台线程）
            self.continue_with_tool_result(msg);
        }
    }
}
```

---

## Phase 5: 配置 UI

### T5.1 Setting 面板扩增 (`ai/settings_panel.rs`)

新增 `Tool Calling` 配置区域：

```
[✓] Enable Tool Calling
    Max rounds per conversation: [10  ]
    Allowed commands: [ps, ls, cat, head  ]
    Read paths: [/home/user, /proc  ]
    [✓] Auto-approve safe operations
```

---

## Phase 6: 收尾

### T6.1 审计日志

在 `db.rs` 新增 `tool_execution_log` 表：

```sql
CREATE TABLE IF NOT EXISTS tool_execution_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp   TEXT NOT NULL DEFAULT (datetime('now')),
    tool_name   TEXT NOT NULL,
    args        TEXT NOT NULL,
    result      TEXT,
    approved    INTEGER NOT NULL DEFAULT 1,
    session_id  TEXT
);
```

### T6.2 安全检查清单

- [ ] 所有 shell 命令有 5s 超时
- [ ] 路径消毒拒绝 `..`、symlink 逃逸
- [ ] `max_tool_rounds` 硬限制，超出自动中断
- [ ] `Safe` 级别工具列表不能由 API 返回的 tool_call 动态指定
- [ ] 审批弹窗不可绕过（键盘快捷键、UI 点击外部）
- [ ] 执行结果截断 4096 字符
- [ ] 工具执行在 `std::thread::spawn` 中，不阻塞 UI
- [ ] 调用失败不影响聊天状态（优雅降级）
- [ ] `ChatMessage` 的 `tool_calls` 字段不会在非 assistant 消息中出现

---

## 实施顺序 & 预估工作量

| Phase | 文件 | 预估行数 | 依赖 |
|---|---|---|---|
| T0.1 扩展 ChatMessage / AiState | `ai/types.rs` | +50 | 无 |
| T0.2 扩展 AiConfig | `ai/types.rs` | +5 | 无 |
| T0.3 扩展 AiStreamEvent | `ai/types.rs` | +5 | 无 |
| T1.2 Tool Registry | `ai/tools/registry.rs` | +80 | T0.1 |
| T1.3 Executors | `ai/tools/executors.rs` | +120 | T1.2 |
| T1.4 Safety Layer | `ai/tools/safety.rs` | +100 | 无 |
| T2.1 streaming tool_calls 拼接 | `ai/client.rs` | +80 | T0.3 |
| T2.2 非 streaming 请求 | `ai/client.rs` | +60 | T0.1 |
| T2.3 Multi-turn loop | `ai/tools/loop.rs` | +100 | T2.1+T2.2 |
| T3.1 替换 ai_pending | `app.rs` | +30 | T0.1 |
| T3.2 重写 send_ai_message | `app.rs` | +50 | T2.3 |
| T3.3 重写 poll_ai_result | `app.rs` | +80 | T3.1 |
| T4.1 审批弹窗 | `ai/chat_panel.rs` | +100 | T3.3 |
| T4.2 审批数据流 | `app.rs` | +30 | T4.1 |
| T5.1 配置 UI | `ai/settings_panel.rs` | +40 | T0.2 |
| T6.1 审计日志 | `db.rs` | +30 | 无 |
| T6.2 安全检查 | 各文件微调 | +20 | 全部 |
| **总计** | | **~980** | |

---

## 验证标准

- [ ] `cargo clippy --release -p live2d-viewer` 零警告
- [ ] `cargo fmt --all` 格式合规
- [ ] 所有工具执行结果在 5s 内返回或超时报错
- [ ] 审批弹窗正常弹出/允许/拒绝/记住
- [ ] 多轮 tool calling 不超过 `max_tool_rounds`
- [ ] 工具调用失败后 LLM 仍能正常回复（graceful degradation）
