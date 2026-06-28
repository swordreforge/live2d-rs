# AI Tool Calling — Architecture Design

> 让 AI 角色能安全地执行文件读取、系统查询、shell 命令等操作。
> 场景：桌面伴侣 AI + 正在运行的游戏/程序 = 能感知用户环境的智能角色。

---

## 1. 设计目标

- **安全第一**：所有工具执行必须经过用户审批 + 白名单 + 沙箱约束
- **多轮自动循环**：LLM 主动决定调用次数，直到获得足够信息再回复
- **最小侵入**：不破坏现有的聊天/记忆/角色卡架构
- **可扩展**：工具按 Plugin 注册，新增工具不改核心流程

---

## 2. 核心术语

| 术语 | 定义 |
|---|---|
| Tool Definition | tool 的 JSON Schema 声明，随 API 请求发送 |
| Tool Call | LLM 返回的调用指令 `{id, function, arguments}` |
| Tool Executor | 本地实际执行操作的函数 |
| Tool Result | 执行结果，作为 `role: "tool"` 消息回传 API |
| Multi-turn Loop | 收到 tool_call → 执行 → 回传 → 再请求 → 直到返回文本 |
| Approval Gate | 用户审批弹窗，拦截高危操作 |

---

## 3. 系统架构

```
┌─────────────────────────────────────────────────────────┐
│                     AppState                             │
│  ┌──────────────────────────────────────────────────┐   │
│  │              AI State Machine                     │   │
│  │  ┌──────┐    ┌─────────┐    ┌──────────────┐    │   │
│  │  │ Idle │───▶│ Waiting │───▶│ PendingTool   │    │   │
│  │  │      │    │ (stream)│    │ (审批弹窗)    │    │   │
│  │  └──────┘    └─────────┘    └──────┬───────┘    │   │
│  │       ▲                ▲          │             │   │
│  │       │                │          ▼             │   │
│  │       │                │    ┌──────────────┐    │   │
│  │       │                └────│  Executing   │    │   │
│  │       │                     │ (线程池执行) │    │   │
│  │       │                     └──────┬───────┘    │   │
│  │       │                            │            │   │
│  │       └────────────────────────────┘            │   │
│  └──────────────────────────────────────────────────┘   │
│                          │                              │
│  ┌───────────────────────┴─────────────────────────┐    │
│  │               Tool Registry                      │    │
│  │  ┌────────────┐ ┌──────────┐ ┌──────────────┐   │    │
│  │  │ read_file  │ │ exec_cmd │ │ web_fetch    │   │    │
│  │  │ read_proc  │ │ list_dir │ │ (future)     │   │    │
│  │  └────────────┘ └──────────┘ └──────────────┘   │    │
│  └──────────────────────────────────────────────────┘    │
│                          │                              │
│  ┌───────────────────────┴─────────────────────────┐    │
│  │               Safety Layer                       │    │
│  │  ┌────────────┐ ┌──────────┐ ┌──────────────┐   │    │
│  │  │ 白名单     │ │ 路径消毒 │ │ 超时/配额    │   │    │
│  │  │ Command    │ │ Path     │ │ Timeout/Rate │   │    │
│  │  └────────────┘ └──────────┘ └──────────────┘   │    │
│  └──────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

---

## 4. 组件设计

### 4.1 AiState 状态机

当前状态 `ai_pending: bool` 扩展为四态枚举：

```rust
pub enum AiState {
    /// 空闲，可接受新输入
    Idle,
    /// 等待 API streaming 回复
    Waiting {
        rx: Receiver<AiStreamEvent>,
        timestamp: f64,
    },
    /// 等待用户审批某 tool 调用
    PendingTool {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        /// 从 "审批前" 到 "审批后重试" 之间需要保留的所有 API 消息
        pending_messages: Vec<ChatMessage>,
    },
    /// 正在本地执行 tool（短同步操作，无等待状态）
    Executing,
}
```

**状态转换图：**

```
Idle ──(send_ai_message)──▶ Waiting ──(stream tool_call)──▶ PendingTool
                              │                                  │
                              │ (stream text)                     │ (用户批准)
                              │                                  ▼
                              │                             Executing
                              │                                  │
                              │                            (结果回传)
                              │                                  │
                              │                            Waiting (续)
                              │                                  │
                              └─────────(Done)──▶ Idle ◀─────────┘
```

### 4.2 ToolDefinition

```rust
/// 一个工具的定义，发送给 API 作为 `tools` 参数。
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema for parameters
    pub parameters: serde_json::Value,
    /// 执行函数
    pub executor: fn(args: serde_json::Value) -> Result<String, String>,
    /// 安全等级
    pub safety_level: SafetyLevel,
}

pub enum SafetyLevel {
    /// 无副作用，自动执行：读 /proc, /sys, read-only file, env
    Safe,
    /// 需要用户弹窗确认：shell 命令、写文件
    Dangerous { description: &'static str },
}
```

### 4.3 Tool Registry

```rust
pub struct ToolRegistry {
    tools: Vec<ToolDefinition>,
}

impl ToolRegistry {
    pub fn builtin() -> Self { ... }
    pub fn definitions(&self) -> Vec<serde_json::Value> { ... }
    pub fn execute(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, ToolError> { ... }
}
```

### 4.4 Safety Layer

安全措施分三层：

| 层 | 措施 | 实现 |
|---|---|---|
| **Pre-call** | 命令白名单 | `exec_cmd` 只允许 `["ps", "ls", "cat", "head", "tail", "grep", "df", "free", "uname", "which", "find", "stat", "lsof", "pgrep"]` |
| **Pre-call** | 路径消毒 | 拒绝包含 `..`、`~`（未展开）、symlink 链指向禁止目录的路径 |
| **Pre-call** | 超时硬限制 | 所有 shell 命令 5s 超时 |
| **Pre-call** | 配额限制 | 每轮对话最多 10 次 tool_call |
| **At-call** | 用户审批 | `Dangerous` 级别弹窗 `[允许] [拒绝] [本次会话永久拒绝]` |
| **Post-call** | 审计日志 | 所有执行记录写入 `tool_execution_log`（内容 + 时间 + 批准人） |
| **Post-call** | 结果截断 | 返回结果超过 4096 字符截断，防止 token 溢出 |

### 4.5 Multi-turn Loop 协议

```
User: "在玩什么游戏？"

API request (tools: [read_file, exec_cmd, ...], tool_choice: "auto")
  ↓
API response: tool_call(id="call_1", fn="exec_cmd", args={cmd: "ps aux | grep -i game"})
  ↓
本地执行 → result = "user 12345 0.5 2.1 123456 78901 ? Ssl 14:30 0:02 ./elden_ring.exe"
  ↓
API request (messages: [...prev..., role:tool, content: result])
  ↓
API response: "你在玩艾尔登法环呢，让我看看你打到哪了——"
  tool_call(id="call_2", fn="exec_cmd", args={cmd: "cat /proc/$(pgrep -f elden_ring)/maps | grep save"})
  ↓
用户审批弹窗 ✅
  ↓
本地执行 → result = ".../elden_ring/save.sl2..."
  ↓
API request (messages: [...prev..., role:tool, content: result])
  ↓
API response: "我看到你的存档了，打到碎星拉塔恩了？那个boss可不简单呢 [happy]"
  (LLM 觉得信息够了，不再调工具)
```

**关键约束：**

- `tool_choice: "auto"` — 让 LLM 自己决定调不调、调几次
- `parallel_tool_calls: false` — 初期禁用并行 tool_call，简化审批流程
- 最大迭代 10 轮，超限强制中断并返回错误
- 每次 tool_call 都过审批（`Safe` 级别可跳过审批）

---

## 5. API 协议适配

### 5.1 请求体变更 (client.rs)

```diff
- "messages": [...],
- "stream": true
+ "messages": [...],
+ "tools": [...],       // ToolDefinition JSON Schema 列表
+ "tool_choice": "auto",
+ "stream": true
```

### 5.2 响应解析变更

当前只解析 `choices[0].delta.content`。需要新增：

```rust
if let Some(tool_calls) = json["choices"][0]["delta"]["tool_calls"].as_array() {
    for tc in tool_calls {
        let id = tc["id"].as_str()...;
        let fn_name = tc["function"]["name"].as_str()...;
        let args = tc["function"]["arguments"].as_str()...;  // delta 可能分片!
        // 需要拼接 delta 分片
    }
}
```

> **注意**：`tool_calls` 在 streaming 模式下可能分多个 delta 到达（`index` 字段标识同一次调用）。需要做 delta 拼接。

### 5.3 role: "tool" 消息格式

```json
{
    "role": "tool",
    "tool_call_id": "call_xxx",
    "content": "执行结果文本"
}
```

---

## 6. 配置文件变更

AiConfig 新增字段：

```rust
pub struct AiConfig {
    // ...现有字段...

    // ── Tool Calling ──
    /// 是否启用 tool calling
    pub tool_calling_enabled: bool,
    /// 无需审批的 Safe 工具列表
    pub auto_approve_tools: Vec<String>,
    /// 每轮对话最大 tool_call 次数
    pub max_tool_rounds: u32,
    /// shell 命令白名单（空=全部需审批）
    pub allowed_commands: Vec<String>,
    /// 允许读取的系统路径前缀
    pub allowed_read_paths: Vec<String>,
}
```

---

## 7. 审批 UI 设计

在 chat_panel 中嵌入弹窗组件：

```
┌─────────────────────────────────┐
│ ⚠ AI 请求执行以下操作            │
│                                 │
│ 命令: ps aux | grep game       │
│ 路径: (无)                      │
│ 等级: ⚠ 危险                   │
│                                 │
│   [✓] 本次允许     [✗] 拒绝    │
│   [本次会话记住]                │
└─────────────────────────────────┘
```

- Safe 操作：`auto_approve_tools` 列表内的自动批准，否则静默执行
- Dangerous 操作：弹出审批窗口，阻塞等待用户决策
- 拒绝时：回传 `"User rejected this operation"` 给 LLM

---

## 8. 事件流程总览（带审批）

```
User sends message
  │
  ▼
build ApiMessages (system + memories + history + current)
  │
  ▼
POST /chat/completions (tools: [...], tool_choice: "auto", stream: true)
  │
  ▼
for each SSE event:
  ├── delta.content → append to assistant response text
  └── delta.tool_calls → append to tool_calls accumulator
  │
  ▼
stream ends (event: [DONE])
  │
  ▼
if tool_calls accumulated:
  ├── for each tool_call:
  │   ├── SafetyLevel::Safe → auto execute
  │   └── SafetyLevel::Dangerous → set PendingTool state → wait for UI approval
  │
  ├── after approval / auto-execute:
  │   ├── execute tool → result string
  │   ├── push role:tool message to messages
  │   └── POST again (without tools) → repeat from "for each SSE event"
  │
  └── max rounds reached → force finish
  │
  ▼
LLM returns text → pop to chat UI
```

---

## 9. 现有架构影响评估

| 模块 | 影响 |
|---|---|
| `ai/types.rs` | +`AiState` 枚举，+`ToolDefinition`，+`AiStreamEvent` 新增 `ToolCall` 变体 |
| `ai/client.rs` | `send_stream()` 需要支持 `tools` 参数 + 解析 `tool_calls` delta |
| `ai/mod.rs` | 新增 `pub mod tools;` |
| `ai/tools/` | 新目录：`mod.rs`, `registry.rs`, `executors.rs`, `safety.rs` |
| `app.rs` | 状态机从 `bool` 改为 `enum`；`poll_ai_result()` 大幅重写 |
| `ai/chat_panel.rs` | 新增审批弹窗 UI 组件 |
| `ai/settings_panel.rs` | 新增 tool calling 配置区 |
| `ai/config.rs` | 序列化 `AiState` 不需要（状态不持久化） |

**无影响**：`memory.rs`, `db.rs`, `tts.rs`, `character_card_panel.rs`

---

## 10. 未来扩展点

- **自定义工具**：通过 `tools/` 目录下的 JSON/YAML 定义动态注册
- **并行 tool_call**：`parallel_tool_calls: true` 并行执行后合并结果
- **Plugin 系统**：类似 OpenCode 的 skill，从外部加载工具定义
- **会话级权限**：用户可以为特定工具设置"本次会话记住"
- **执行结果缓存**：相同参数的同一工具短时间内命中缓存
