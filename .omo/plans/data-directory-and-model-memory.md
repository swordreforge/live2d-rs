# Plan: User Data Directory + SQLite Model Memory

## 目标

1. **用户数据目录**：为 `live2d-viewer` 创建标准的 XDG 用户数据目录，存储 SQLite 数据库
2. **模型文件记忆**：通过 SQLite 记录模型历史（路径、名称、版本、缩放），支持下次启动自动恢复

## 设计决策（来自用户确认）

| 问题 | 决策 |
|------|------|
| 模型历史如何记录 | CLI 传参 / GUI Add Model 自动加入历史 |
| 每模型存储内容 | 名称 + 模型版本(V2/V3) + 文件路径 + 缩放级别（可选，方便记忆） |
| V2 支持 | 同等支持 |
| SQLite | rusqlite bundled feature |
| 重复模型去重 | 同一路径自动 UPSERT（更新 name/zoom_scale/last_opened），不产生重复记录 |

## 数据库 Schema

```sql
CREATE TABLE IF NOT EXISTS global_settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);
-- 全局设置：last_active_model_path, pet_mode, auto_play_idle, window_width, window_height

CREATE TABLE IF NOT EXISTS model_history (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path    TEXT NOT NULL UNIQUE,
    name         TEXT NOT NULL,
    model_version TEXT NOT NULL,  -- 'V2' or 'V3'
    zoom_scale   REAL,            -- NULL if not set
    last_opened  TEXT NOT NULL DEFAULT (datetime('now')),
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
-- zoom_scale: V3 对应 Camera scale_x/scale_y 的均值，V2 对应 v2_scale
-- 同一模型路径仅保留一条记录（UPSERT）。重复加载 → 更新 name/zoom_scale/last_opened，不插入新行。
```

## 实施步骤

### Step 1: 新增依赖 + 数据目录模块

**涉及文件：**
- `live2d-viewer/Cargo.toml`：加 `rusqlite = { version = "0.31", features = ["bundled"] }`, `dirs = "5"`
- `live2d-viewer/src/data_dir.rs`：新建模块

**`data_dir.rs` 职责：**
- 确定数据目录路径：`$XDG_DATA_HOME/live2d-rs/`（Linux）→ 回退 `~/.local/share/live2d-rs/`
- `ensure_data_dir()`：创建目录（如果不存在）
- `db_path()`：返回 `{data_dir}/state.db` 完整路径
- `AppDataDir` 结构体持有 path，在 `main.rs` 启动时初始化

### Step 2: DB 模块 — 建表 + CRUD

**涉及文件：**
- `live2d-viewer/src/db.rs`：新建模块

**`db.rs` 职责：**
- `AppDb` 结构体持有 `rusqlite::Connection`
- `AppDb::open(path)`：打开/创建 DB，运行 `CREATE TABLE IF NOT EXISTS`
- **global_settings**: `get_setting(key) -> Option<String>`, `set_setting(key, value)`
- **model_history**（同一模型路径只保留一条记录）: 
  - `add_or_update_model(file_path, name, model_version, zoom_scale)` — UPSERT on `file_path`（同路径→更新已有行，不新增）
  - `get_model_history() -> Vec<ModelRecord>` — 按 `last_opened DESC`
  - `get_model_by_path(file_path) -> Option<ModelRecord>`
  - `update_zoom(path, zoom_scale)`
  - `update_last_opened(path)`
  - `remove_model(path)`

`ModelRecord` 结构体：

```rust
pub struct ModelRecord {
    pub file_path: String,
    pub name: String,
    pub model_version: String,
    pub zoom_scale: Option<f32>,
    pub last_opened: String,
}
```

### Step 3: main.rs 启动时初始化

**涉及文件：**
- `live2d-viewer/src/main.rs`

**改动：**
- `main()` 开头：`let data_dir = data_dir::ensure_data_dir()?; let db = db::AppDb::open(data_dir.db_path())?;`
- GL 初始化后：从 DB 恢复全局设置
- 从 DB 恢复模型历史列表：读取 `model_history` 表填充 `app.model_list`
- 恢复上次活动模型（如果有 `last_active_model_path` 设置且该目录存在）
- 退出前不需要显式保存（每次操作实时写 DB）

### Step 4: AppState 集成 — 模型历史自动追踪

**涉及文件：**
- `live2d-viewer/src/app.rs`
- `live2d-viewer/src/gui.rs`

**AppState 新增：**
- `app_db: Option<AppDb>` 或通过 `Rc<Mutex<AppDb>>` 共享
- `add_model_dir(path)` 自动调用 `app_db.add_or_update_model(...)`
- `switch_to(idx)` / `begin_switch(idx)` 自动更新 `last_opened`
- `start_motion` / `pet_mode` 等变化自动更新 `global_settings`
- GUI "Add Model..." 按钮后自动调用 `add_or_update_model`

**模型列表恢复流程：**
1. `main.rs` 启动时：`let records = db.get_model_history()`
2. 对每条记录，检查 `file_path` 目录是否仍存在
3. 存在的目录 → `detect_model_format()` → 填入 `app.model_list`
4. 不存在的目录 → 从 DB 删除（清理孤立记录）

### Step 5: 缩放级别保存/恢复

**涉及文件：**
- `live2d-viewer/src/main.rs`（事件循环）
- `live2d-viewer/src/app.rs`

**保存时机：**
- V3: camera 变化时（`fit_to_canvas`、`zoom`、`pan` 调用后）→ 保存 `camera.scale_x` 均值
- V2: `v2_scale` 变化时 → 直接保存
- 每帧不存，只在变化时通过 flag 触发写入（防高频写入）

**恢复时机：**
- `switch_to` / `complete_v3_switch` 完成后，从 DB 读取该模型的 `zoom_scale`
- V3: `scale_x = scale_y = zoom_scale; translate_x = translate_y = 0;`
- 设置 `camera_needs_fit = false`（跳过 fit_to_canvas 覆盖）

### Step 6: V2 支持

**涉及文件：**
- `live2d-viewer/src/app.rs`（`switch_to` V2 分支）
- `live2d-viewer/src/main.rs`

V2 缩放保存/恢复方式与 V3 相同，使用 `v2_scale` 字段。

### Step 7（可选）：GUI 设置窗口

**涉及文件：**
- `live2d-viewer/src/gui.rs`

在 GUI 中添加简单入口：
- 显示数据目录路径
- 清空模型历史按钮（`DELETE FROM model_history`）
- 显示/编辑 pet_mode、auto_play_idle 偏好

## 表结构（最终）

```
global_settings
├── last_active_model_path → "/path/to/last/model"
├── pet_mode → "true"/"false"
├── auto_play_idle → "true"/"false"
├── window_width → "1200"
└── window_height → "800"

model_history
├── id (PK AUTOINCREMENT)
├── file_path (UNIQUE NOT NULL)  — 模型目录绝对路径
├── name (NOT NULL)              — 目录名
├── model_version (NOT NULL)     — "V2" / "V3"
├── zoom_scale (REAL)            — 缩放值，V3: scale_x均值，V2: v2_scale
├── last_opened (TEXT)           — datetime('now')
└── created_at (TEXT)            — datetime('now')
```

## 实现顺序

```
Step 1: 依赖 + data_dir 模块
    ↓
Step 2: DB 模块（建表 + CRUD）
    ↓
Step 3: main.rs 启动初始化 + 全局设置恢复
    ↓
Step 4: AppState 集成 + 模型历史自动追踪
    ↓
Step 5: 缩放级别保存/恢复
    ↓
Step 6: V2 支持
    ↓
(可选) Step 7: GUI 设置窗口
```

## 文件变更清单

| 文件 | 变更类型 |
|------|----------|
| `live2d-viewer/Cargo.toml` | 修改：+rusqlite, +dirs |
| `live2d-viewer/src/data_dir.rs` | 新建 |
| `live2d-viewer/src/db.rs` | 新建 |
| `live2d-viewer/src/main.rs` | 修改：初始化 data_dir + DB，启动恢复 |
| `live2d-viewer/src/app.rs` | 修改：AppDB 集成，模型历史追踪，缩放保存/恢复 |
| `live2d-viewer/src/gui.rs` | 修改（可选）：设置入口 |
