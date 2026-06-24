# Wayland 原生 Pet Mode (layer-shell) 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Wayland 上进入 pet mode 时，创建独立 sctk layer-shell 表面（Layer::Top）显示 Live2D 模型，实现真正的总在上方。

**Architecture:** 主线程保持 winit 窗口（正常模式）。Wayland 上进入 pet mode 时，spawn 独立线程运行 sctk 事件循环 + 独立 GL context，layer-shell 表面以 Layer::Top 级别浮在所有窗口之上。退出 pet mode 时销毁表面、join 线程。X11 / GNOME 走原有路径（set_window_level）。

**Tech Stack:** winit 0.29, glutin 0.31, glow, smithay-client-toolkit 0.20, wayland-client 0.31, wayland-protocols-wlr 0.3

---

## 架构概览

```
┌─ 主线程 ─────────────────────────────────────┐
│  winit EventLoop                               │
│  app::AppState                                 │
│    ├─ pet_thread: Option<JoinHandle>           │
│    └─ pet_cmd_tx: Sender<PetCommand>           │
│  pet_mode_changed → Wayland 路径:              │
│    spawn pet_thread                            │
│    hide/resize winit window                    │
│                                                │
│  AboutToWait: 检查 pet_event_rx.try_recv()     │
│    → Configured(w,h) / Error / Exited          │
└──────────────┬─────────────────────────────────┘
               │ PetCommand::EnterPetMode { model_dir }
               │ PetCommand::ExitPetMode
               └──────────────────────────────────┐
┌─ Pet Thread ────────────────────────────────────┘
│  wayland_client::Connection::connect_to_env()
│  smithay-client-toolkit 事件循环 (calloop)
│  ├─ zwlr_layer_shell_v1 → Layer::Top
│  ├─ wl_compositor → wl_surface
│  ├─ glutin::Display + Context (独立 GL)
│  ├─ glow::Context
│  └─ 模型加载 (从 model_dir 读取)
│  循环:
│    1. dispatch Wayland events
│    2. receive commands (非阻塞)
│    3. advance motion
│    4. render model
│    5. swap buffers
│    6. sleep 至下一帧
└────────────────────────────────
```

### 线程间通信协议

```rust
/// 主线程 → Pet 线程命令
enum PetCommand {
    Enter {
        model_dir: PathBuf,
        model_format: ModelFormat,
    },
    Exit,
}

/// Pet 线程 → 主线程事件
enum PetEvent {
    Configured { width: u32, height: u32 },
    Error(String),
    Exited,
}
```

### 文件变更清单

| 文件 | 操作 | 说明 |
|---|---|---|
| `live2d-viewer/Cargo.toml` | 修改 | 添加 sctk 依赖 |
| `live2d-viewer/src/wayland_pet.rs` | **新建** | sctk 表面创建 + GL 渲染 + 线程管理 |
| `live2d-viewer/src/app.rs` | 修改 | 添加 pet_thread handle, GNOME 检测, 通信 channel |
| `live2d-viewer/src/main.rs` | 修改 | Wayland pet mode 进入/退出逻辑 |

---

### Task 1: 添加 sctk 依赖 + 检测 GNOME

**Files:**
- Modify: `live2d-viewer/Cargo.toml`
- Modify: `live2d-viewer/src/app.rs`

**Step 1.1: Cargo.toml — 添加 platform-specific 依赖**

在 `[target.'cfg(target_os = "linux")'.dependencies]` 段添加：

```toml
# wayland_pet.rs 需要 (仅 Wayland 场景，X11 场景不编译)
smithay-client-toolkit = { version = "0.20", default-features = false, features = [
    "calloop",
] }
```

注意：`wayland-client 0.31.14`、`wayland-protocols-wlr 0.3.12`、`calloop 0.14.4` 已作为传递依赖存在于 Cargo.lock 中， sctk 0.20 会复用这些版本。

**Step 1.2: app.rs — GNOME 检测函数**

在 `impl AppState` 前添加：

```rust
/// 检测是否运行在 GNOME 上（GNOME 不支持 layer-shell）。
pub fn is_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_lowercase().contains("gnome"))
        .unwrap_or(false)
}
```

**Step 1.3: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过（仅验证依赖树正确）。

---

### Task 2: `wayland_pet.rs` 模块骨架 — 数据结构 + 线程生命周期

**Files:**
- Create: `live2d-viewer/src/wayland_pet.rs`
- Modify: `live2d-viewer/src/main.rs` 添加 mod 声明

**Step 2.1: 线程间通信类型**

```rust
// live2d-viewer/src/wayland_pet.rs

use std::path::PathBuf;
use std::sync::mpsc;

/// 主线程 → Pet 线程命令
pub enum PetCommand {
    Enter {
        model_dir: PathBuf,
        model_format: crate::app::ModelFormat,
    },
    Exit,
}

/// Pet 线程 → 主线程事件
pub enum PetEvent {
    Configured { width: u32, height: u32 },
    Error(String),
    Exited,
}
```

**Step 2.2: `spawn_pet_surface()` 函数签名**

```rust
/// 在独立线程上创建 sctk layer-shell 表面 + GL 上下文。
///
/// 返回线程句柄和事件接收端。
/// 调用者通过 `cmd_tx` 发送命令控制线程生命周期。
pub fn spawn_pet_surface(
    cmd_rx: mpsc::Receiver<PetCommand>,
    event_tx: mpsc::Sender<PetEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // 后续 Task 在此实现
        log::info!("[pet/wayland] thread started");
        // 等待 Enter 命令
        loop {
            match cmd_rx.recv() {
                Ok(PetCommand::Enter { model_dir, model_format }) => {
                    log::info!("[pet/wayland] enter: {:?}", model_dir);
                    // Task 3~5 将在此处展开
                    break;
                }
                Ok(PetCommand::Exit) | Err(_) => {
                    log::info!("[pet/wayland] exited before enter");
                    return;
                }
            }
        }
        // 事件和渲染循环
        log::info!("[pet/wayland] thread ended");
    })
}
```

**Step 2.3: main.rs 添加 mod**

在 `live2d-viewer/src/main.rs` 开头加：

```rust
pub mod wayland_pet;
```

注意：`wayland_pet.rs` 使用 `cfg(target_os = "linux")` 保护所有内容（但 Rust 允许无条件 mod 声明，Linux-only 内容在 cfg 块内）。为避免 unsued import 警告，可选加 `#[cfg(target_os = "linux")]`。

```rust
#[cfg(target_os = "linux")]
pub mod wayland_pet;
```

**Step 2.4: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过，无 warning。

---

### Task 3: 创建 sctk 连接 + layer-shell 表面

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs`

**Step 3.1: PetSurface 结构体**

在 `spawn_pet_surface()` 的线程闭包内构建设置。

```rust
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell,
};
use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::seat::SeatState;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
```

```rust
struct PetState {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    configured_size: Option<(u32, u32)>,
}

impl CompositorHandler for PetState {
    fn compositor_state(&mut self) -> &mut smithay_client_toolkit::compositor::CompositorState {
        unimplemented!("will be filled in step 3.2");
    }
}

impl ProvidesRegistryState for PetState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState,];
}
```

**Step 3.2: 创建连接 + layer-surface**

```rust
fn setup_pet_surface(
    cmd_rx: &mpsc::Receiver<PetCommand>,
    event_tx: &mpsc::Sender<PetEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    let globals = registry_queue_init::<PetState>(&conn)?;
    let mut state = PetState {
        registry_state: globals.registry_state(),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        configured_size: None,
    };

    let compositor = globals
        .bind::<smithay_client_toolkit::compositor::CompositorState, _>(&qh, ())?;
    let layer_shell = globals.bind::<LayerShell, _>(&qh, ())?;

    let surface = compositor.create_surface(&qh);
    let layer_surface = layer_shell.create_layer_surface(
        &qh,
        surface.clone(),
        Layer::Top,
        Some("live2d-pet"),
        None,
    );
    layer_surface.set_size(400, 500); // 初始尺寸，后续根据模型调整
    layer_surface.set_anchor(Anchor::BOTTOM | Anchor::RIGHT);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    surface.commit();
    event_queue.roundtrip(&mut state)?;

    // 通知主线程表面已就绪
    if let Some((w, h)) = state.configured_size {
        let _ = event_tx.send(PetEvent::Configured { width: w, height: h });
    }

    // 进入事件循环（Task 5 填充）
    run_event_loop(&mut state, &mut event_queue, &cmd_rx, &event_tx)?;

    Ok(())
}
```

**Step 3.3: 分发表面 configure 事件**

```rust
impl Dispatch<smithay_client_toolkit::shell::wlr_layer::ZwlrLayerSurfaceV1, ()>
    for PetState
{
    fn event(
        state: &mut Self,
        _proxy: &smithay_client_toolkit::shell::wlr_layer::ZwlrLayerSurfaceV1,
        event: <smithay_client_toolkit::shell::wlr_layer::ZwlrLayerSurfaceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use smithay_client_toolkit::shell::wlr_layer::Event;
        match event {
            Event::Configure {
                width,
                height,
                ..
            } => {
                if width > 0 && height > 0 {
                    state.configured_size = Some((width, height));
                }
            }
            Event::Closed => {
                log::info!("[pet/wayland] layer surface closed by compositor");
            }
            _ => {}
        }
    }
}
```

其他必要的 `Dispatch` 实现：

```rust
impl Dispatch<WlSurface, ()> for PetState {}
impl Dispatch<smithay_client_toolkit::compositor::CompositorState, ()> for PetState {}
impl Dispatch<LayerShell, ()> for PetState {}
```

**Step 3.4: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过。如有缺少的 Dispatch trait 实现，按编译器提示补充空实现 `impl Dispatch<ProtocolType, ()> for PetState {}`。

---

### Task 4: GL context + glow 渲染

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs`

**Step 4.1: 创建 glutin Display + Context**

在 `setup_pet_surface()` 中，创建完 layer-surface 后：

```rust
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext};
use glutin::display::{Display, DisplayApiPreference};
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, WindowSurface};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use std::num::NonZeroU32;
use std::ffi::c_void;
```

```rust
// 从 Wayland 连接获取 display 指针
let display_ptr = conn.backend().display_ptr() as *mut c_void;
let mut raw_display_handle = WaylandDisplayHandle::empty();
raw_display_handle.display = NonNull::new(display_ptr).unwrap().as_ptr();
let raw_display = RawDisplayHandle::Wayland(raw_display_handle);

let gl_display = unsafe { Display::new(raw_display, DisplayApiPreference::Egl)? };

let template = ConfigTemplateBuilder::new().with_alpha_size(8).build();
let gl_config = gl_display
    .find_configs(template)?
    .next()
    .ok_or_else(|| anyhow::anyhow!("no suitable GL config"))?;

// 从 wl_surface 获取 window 指针
let surface_ptr = surface.id().as_ptr() as *mut c_void;
let mut raw_window_handle = WaylandWindowHandle::empty();
raw_window_handle.surface = NonNull::new(surface_ptr).unwrap().as_ptr();
let raw_window = RawWindowHandle::Wayland(raw_window_handle);

let context_attrs = ContextAttributesBuilder::new().build(Some(raw_window));
let not_current = unsafe { gl_display.create_context(&gl_config, &context_attrs)? };

let (init_w, init_h) = state.configured_size.unwrap_or((400, 500));
let phys_w = NonZeroU32::new(init_w.max(1)).unwrap();
let phys_h = NonZeroU32::new(init_h.max(1)).unwrap();
let surf_attrs = SurfaceAttributesBuilder::<WindowSurface>::new()
    .build(raw_window, phys_w, phys_h);
let egl_surface = unsafe { gl_display.create_window_surface(&gl_config, &surf_attrs)? };

let gl_context = unsafe { not_current.make_current(&egl_surface)? };
// swap interval 需要 EGL — 略过，后续在空转中控制帧率

// glow::Context
let gl = unsafe {
    glow::Context::from_loader_function(|s| {
        let c_str = std::ffi::CString::new(s).expect("gl proc name");
        gl_display.get_proc_address(&c_str) as *const _
    })
};

// 清理测试
unsafe {
    gl.clear_color(0.0, 0.0, 0.0, 0.0);
    gl.clear(glow::COLOR_BUFFER_BIT);
}
egl_surface.swap_buffers(&gl_context)?;
```

注意：需要使用 `raw-window-handle = "0.5"` 的 API。当前项目中已经通过 `winit` 的 `rwh_05` feature 引入。确认 `Cargo.toml` 中 `raw-window-handle` 的版本约束：

```toml
raw-window-handle = "0.5"  # 如果 Cargo.toml 没有，加在 [dependencies] 里
```

**Step 4.2: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过。如 raw-window-handle API 签名不同（0.5 vs 0.6），按实际 API 调整（`new()` / `empty()` + field assignment）。

---

### Task 5: 事件循环 + 线程生命周期

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs`

**Step 5.1: `run_event_loop()` 实现**

```rust
fn run_event_loop(
    state: &mut PetState,
    event_queue: &mut wayland_client::EventQueue<PetState>,
    cmd_rx: &mpsc::Receiver<PetCommand>,
    event_tx: &mpsc::Sender<PetEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 帧率控制：60fps → ~16ms 每帧
    let frame_duration = std::time::Duration::from_secs_f64(1.0 / 60.0);

    loop {
        let frame_start = std::time::Instant::now();

        // 1. 处理 Wayland 事件
        event_queue.dispatch_pending(state)?;

        // 2. 检查主线程命令（非阻塞）
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PetCommand::Exit => {
                    let _ = event_tx.send(PetEvent::Exited);
                    return Ok(());
                }
                PetCommand::Enter { .. } => {
                    // 已在运行中，忽略
                }
            }
        }

        // 3. 渲染（占位 — 后续 Task 填充实际模型渲染）
        // 当前清除为透明色
        // 当模型加载完成后，在此处调用 motion advance + render

        // 4. 请求 frame callback
        // Wayland 层：surface.frame() 可以获得 vsync 信号
        // 但初次实现使用 sleep 控制帧率

        // 5. Swap buffers
        // egl_surface.swap_buffers(&gl_context)?;

        // 6. 帧率控制
        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        }
    }
}
```

**Step 5.2: 整合 spawn_pet_surface**

将 `setup_pet_surface()` 包装到 `spawn_pet_surface()` 中：

```rust
pub fn spawn_pet_surface(
    cmd_rx: mpsc::Receiver<PetCommand>,
    event_tx: mpsc::Sender<PetEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("wayland-pet".into())
        .spawn(move || {
            // 等待 Enter 命令
            match cmd_rx.recv() {
                Ok(PetCommand::Enter { model_dir: _, model_format: _ }) => {
                    log::info!("[pet/wayland] received enter, setting up surface");
                    if let Err(e) = setup_pet_surface(&cmd_rx, &event_tx) {
                        log::error!("[pet/wayland] setup failed: {e}");
                        let _ = event_tx.send(PetEvent::Error(e.to_string()));
                    }
                }
                Ok(PetCommand::Exit) | Err(_) => {
                    log::info!("[pet/wayland] exited before enter");
                }
            }
            log::info!("[pet/wayland] thread ended");
            let _ = event_tx.send(PetEvent::Exited);
        })
        .expect("spawn wayland pet thread")
}
```

**Step 5.3: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过。

---

### Task 6: 主线程接线 — app.rs 状态 + main.rs 生命周期

**Files:**
- Modify: `live2d-viewer/src/app.rs`
- Modify: `live2d-viewer/src/main.rs`

**Step 6.1: app.rs — pet 线程状态字段**

在 `AppState` 中添加：

```rust
use std::sync::mpsc;

// ... 在 struct AppState 的字段区域 ...
#[cfg(target_os = "linux")]
pub pet_wayland_cmd_tx: Option<mpsc::Sender<wayland_pet::PetCommand>>,
#[cfg(target_os = "linux")]
pub pet_wayland_event_rx: Option<mpsc::Receiver<wayland_pet::PetEvent>>,
#[cfg(target_os = "linux")]
pub pet_wayland_thread: Option<std::thread::JoinHandle<()>>,
#[cfg(not(target_os = "linux"))]
pub pet_wayland_cmd_tx: Option<()>,
#[cfg(not(target_os = "linux"))]
pub pet_wayland_event_rx: Option<()>,
#[cfg(not(target_os = "linux"))]
pub pet_wayland_thread: Option<()>,
```

在 `AppState::new()` 中初始化：

```rust
#[cfg(target_os = "linux")]
{
    pet_wayland_cmd_tx: None,
    pet_wayland_event_rx: None,
    pet_wayland_thread: None,
}
#[cfg(not(target_os = "linux"))]
{
    pet_wayland_cmd_tx: None,
    pet_wayland_event_rx: None,
    pet_wayland_thread: None,
}
```

**Step 6.2: main.rs — Wayland pet mode 进入/退出**

修改 `pet_mode_changed` 处理块（当前 main.rs L291-317）：

```rust
// 替换原有：
// window.set_decorations(false);
// window.set_window_level(WindowLevel::AlwaysOnTop);
// 为：

if app.pet_mode {
    let use_wayland_pet = on_wayland
        && !crate::app::is_gnome()
        && cfg!(target_os = "linux");
    if use_wayland_pet {
        // ── Wayland 原生路径：spawn sctk layer-shell ──
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = wayland_pet::spawn_pet_surface(cmd_rx, event_tx);

        app.pet_wayland_cmd_tx = Some(cmd_tx);
        app.pet_wayland_event_rx = Some(event_rx);
        app.pet_wayland_thread = Some(handle);

        // 通知 pet 线程进入模式
        if let Some(ref tx) = app.pet_wayland_cmd_tx {
            let format = ...; // 从 app 获取当前模型格式
            let dir = ...;    // 从 app 获取当前模型路径
            let _ = tx.send(wayland_pet::PetCommand::Enter {
                model_dir: dir,
                model_format: format,
            });
        }

        // 隐藏主窗口（或者缩成小点）
        window.set_visible(false);
    } else {
        // ── X11 / GNOME 原有路径 ──
        window.set_decorations(false);
        window.set_window_level(WindowLevel::AlwaysOnTop);
        // ...
    }
} else {
    // 退出 pet mode
    if app.pet_wayland_cmd_tx.is_some() {
        // 通知 pet 线程退出
        if let Some(tx) = app.pet_wayland_cmd_tx.take() {
            let _ = tx.send(wayland_pet::PetCommand::Exit);
        }
        // join 线程
        if let Some(handle) = app.pet_wayland_thread.take() {
            let _ = handle.join();
        }
        app.pet_wayland_event_rx = None;

        // 显示主窗口
        window.set_visible(true);
        window.set_decorations(true);
        window.set_window_level(WindowLevel::Normal);
    } else {
        // X11 原有路径
        window.set_decorations(true);
        window.set_window_level(WindowLevel::Normal);
    }
}
```

**Step 6.3: AboutToWait 中轮询 pet 线程事件**

在 `Event::AboutToWait` 中，添加：

```rust
// 检查 pet 线程事件
#[cfg(target_os = "linux")]
if let Some(ref rx) = app.pet_wayland_event_rx {
    while let Ok(event) = rx.try_recv() {
        match event {
            wayland_pet::PetEvent::Configured { width, height } => {
                log::info!("[pet/wayland] surface configured: {width}x{height}");
            }
            wayland_pet::PetEvent::Error(e) => {
                log::error!("[pet/wayland] error: {e}");
                // 回退：显示主窗口
                window.set_visible(true);
            }
            wayland_pet::PetEvent::Exited => {
                log::info!("[pet/wayland] thread exited");
            }
        }
    }
}
```

**Step 6.4: 获取当前模型路径和格式**

在 pet mode 进入点，需要知道当前模型目录。在 `AppState` 中添加 getter：

```rust
impl AppState {
    pub fn current_model_dir(&self) -> Option<PathBuf> {
        self.current_idx.and_then(|i| self.model_list.get(i)).map(|e| e.dir.clone())
    }
    pub fn current_model_format(&self) -> Option<ModelFormat> {
        self.current_idx.and_then(|i| self.model_list.get(i)).and_then(|e| e.format)
    }
}
```

**Step 6.5: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过。

---

### Task 7: 模型加载 + 渲染在 pet 线程

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs`

**Step 7.1: V3 模型加载**

在 `setup_pet_surface()` 中，创建 GL context 后，从 model_dir 加载模型：

```rust
// 读取 model3.json
let model_path = model_dir.join("model3.json");
let model3_json_bytes = std::fs::read(&model_path)
    .map_err(|e| anyhow::anyhow!("read model3.json: {e}"))?;
let model3_json = crate::model_loader::parse_model3_json(&model3_json_bytes)
    .map_err(|e| anyhow::anyhow!("parse model3.json: {e}"))?;

// 读取 MOC3
let moc_path = model_dir.join(&model3_json.file_references.moc);
let moc_data = std::fs::read(&moc_path)
    .map_err(|e| anyhow::anyhow!("read moc3: {e}"))?;

// 创建 Moc + Model (使用现有 live2d-core API)
use live2d_core::Moc;
let moc = Moc::from_bytes(&moc_data)
    .map_err(|e| anyhow::anyhow!("Moc::from_bytes: {e}"))?;
let mut model = moc.initialize_model()
    .map_err(|e| anyhow::anyhow!("Model::initialize: {e}"))?;

// 加载纹理（使用现有 texture::load_texture）
let mut textures = Vec::new();
for tex_path in &model3_json.file_references.textures {
    let full_path = model_dir.join(tex_path);
    if let Ok(data) = std::fs::read(&full_path) {
        if let Ok(tex) = unsafe { crate::texture::load_texture(&gl, &data) } {
            textures.push(tex);
        }
    }
}

// 初始化渲染器（简化版 — 仅使用 V3 renderer）
let mut renderer = unsafe {
    crate::renderer::Live2dRenderer::new(&gl)
        .map_err(|e| anyhow::anyhow!("renderer: {e}"))?
};

renderer.textures = textures;
model.update();
```

**Step 7.2: 初始化 motion 系统**

```rust
let mut motion_queue = crate::motion::MotionQueueManager::new();
let mut eye_blink = crate::motion::eye_blink::EyeBlink::new();
let mut breath = crate::motion::breath::Breath::new();
// 加载 idle motion...
```

加载 idle motion 的细节与 `app.rs` 中现有逻辑类似：从 model3.json 读取 motions、构建 `MotionQueue`。初次实现可以跳过 motion 系统，直接渲染静态模型。后续迭代中逐步补齐。

**Step 7.3: 渲染循环**

```rust
// 在 run_event_loop 的帧循环中
let size = state.configured_size.unwrap_or((400, 500));

// 视口
unsafe {
    gl.viewport(0, 0, size.0 as i32, size.1 as i32);
    gl.clear_color(0.0, 0.0, 0.0, 0.0);
    gl.clear(glow::COLOR_BUFFER_BIT);
}

// advance motion + update model
// renderer.render(&gl, &model, &camera);

egl_surface.swap_buffers(&gl_context)?;
```

**Step 7.4: Camera 在 pet 窗口**

pet 窗口需要独立的 Camera 实例（因为窗口尺寸不同）。在 `PetState` 中添加 `camera: Camera`。

**Step 7.5: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过。

---

### Task 8: V2 模型支持

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs`

**Step 8.1: 分路径加载**

根据 `model_format` 选择 V2/V3 加载路径：

```rust
match model_format {
    crate::app::ModelFormat::V3 => {
        // Task 7 的 V3 路径
    }
    crate::app::ModelFormat::V2 => {
        // V2 路径
        let model_json = model_dir.join("model.json"); // 或 model0.json
        let v2 = crate::model_loader::load_v2_model(&model_dir, &model_json)?;
        // V2 使用 live2d_v2_core::gl_init + v2.draw()
    }
}
```

**Step 8.2: V2 渲染**

V2 需要：
1. `live2d_v2_core::gl_init()` — 但已在主线程调用过，不能重复调用
2. V2 使用 `v2.draw()` 直接渲染

需要在 pet 线程初始化 V2 的 GL 函数。V2 使用 `glad` 库，它的初始化是全局的。重复调用 `gl_init()` 应安全（返回 0 但已初始化过）。

```rust
let v2_vao = unsafe { gl.create_vertex_array().expect("create V2 VAO") };
// ... 渲染时绑定 VAO，调用 v2.draw()
```

**Step 8.3: 验证编译**

```bash
cargo check --release -p live2d-viewer
```

预期：编译通过。

---

### Task 9: 空转测试 + Wayland compositor 验证

**Step 9.1: 构建 release 二进制**

```bash
cargo build --release -p live2d-viewer
```

**Step 9.2: 在 Sway / Hyprland / KDE 上手动测试**

```bash
# 在 Wayland session 中（不设 WINIT_UNIX_BACKEND）
# 或者
WAYLAND_DISPLAY=wayland-1 WINIT_UNIX_BACKEND=wayland ./target/release/live2d-viewer /path/to/model
```

- 验证主窗口正常显示
- 进入 pet mode → layer-shell surface 出现（总在上方）
- 退出 pet mode → layer-shell surface 消失，主窗口恢复
- 验证点击穿透（`set_cursor_hittest(false)`）在 pet 窗口上工作
- 验证没有 X11 依赖错误

**Step 9.3: 在 GNOME 上验证回退**

```bash
XDG_CURRENT_DESKTOP=GNOME ./target/release/live2d-viewer /path/to/model
```

- 验证 pet mode 使用旧路径（window.set_level）而不是 layer-shell
- 应有一条 log: `[pet] GNOME detected, falling back to xdg-shell path`

---

### Task 10: 错误处理 + 安全清理

**Step 10.1: 线程 panic 安全**

`spawn_pet_surface` 中的 `thread::spawn` 应捕获 panic 并通过 `PetEvent::Error` 通知主线程：

```rust
let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
    setup_pet_surface(&cmd_rx, &event_tx)
}));
match result {
    Ok(Ok(())) => {}
    Ok(Err(e)) => {
        log::error!("[pet/wayland] setup error: {e}");
        let _ = event_tx.send(PetEvent::Error(e.to_string()));
    }
    Err(panic) => {
        let msg = if let Some(s) = panic.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".into()
        };
        log::error!("[pet/wayland] thread panic: {msg}");
        let _ = event_tx.send(PetEvent::Error(msg));
    }
}
```

**Step 10.2: Drop 顺序**

确保 GL context 在 wl_surface 之前 drop，egl_surface 在 context 之前：

```rust
// 手动 drop（Rust 的 drop 顺序是声明反序，但显式更安全）
drop(gl_context);     // 先释放 GL context（释放 EGL 资源）
drop(egl_surface);   // 再释放 EGL surface
drop(surface);       // 最后释放 wl_surface
```

**Step 10.3: 主线程超时 join**

在 main.rs 中 join pet 线程时设置超时：

```rust
if let Some(handle) = app.pet_wayland_thread.take() {
    // 给 pet 线程 2 秒退出
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while deadline > std::time::Instant::now() {
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !handle.is_finished() {
        log::warn!("[pet/wayland] thread did not exit in time, detaching");
    }
    // JoinHandle 的 drop 会 detach 线程
}
```

**Step 10.4: 验证编译**

```bash
cargo check --release -p live2d-viewer
cargo clippy --release -p live2d-viewer
```

预期：零 error，零 warning。

---

## 风险项与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| **GL context 跨线程**：glutin 的 Display 和 Context 不能跨线程 Move？ | 中 | 高 | glutin 0.31 的 Context 实现了 Send？如果未实现，需要 Arc<Mutex<>> 或所有权重新设计。实际情况：它们在同一线程创建和使用（pet 线程），不跨线程 move。 |
| **V2 gl_init 不能多次调用** | 高 | 中 | glad 的 `gl_init()` 是全局单例。在 pet 线程调用时返回 0（已初始化）。需要测试。 |
| **GNOME 检测不准确** | 低 | 低 | `XDG_CURRENT_DESKTOP` 也可能为 `"GNOME-Flashback"` 等变体。用 `.contains("gnome")` 覆盖常见情况。 |
| **sctk 事件队列阻塞**：event_queue.dispatch_pending 不阻塞，但如果没有事件，空转的 sleep 不够精确。 | 低 | 低 | 使用 Wayland frame callback 优化帧同步。初次实现用 sleep 控制帧率即可。 |
| **快捷键/输入转发**：pet 模式不需要键盘输入（KeyboardInteractivity=None），但如果将来需要交互，需要调整。 | 中 | 低 | 当前明确留为 None。如需交互，后续可以设 OnDemand + 非空输入区域。 |
| **模型一致性问题**：主线程和 pet 线程各自加载模型，状态可能不同步（参数值、motion 进度）。 | 高 | 中 | 初始实现让两个实例独立。后续可以通过 PetCommand::UpdateParameters(values) 同步。 |

---

## 未纳入范围（Future Work）

1. **egui 在 pet 表面**：使用 Ciantic/egui-wgpu-example-smithay 模式在 pet 表面渲染 egui UI（宠物工具栏、参数调节等）
2. **输入交互**：在 pet 表面上设置非空输入区域，实现模型头跟随、tap 交互
3. **模型参数同步**：通过通道将主线程的参数值定期同步到 pet 线程
4. **多显示器处理**：layer-shell 的 anchor/size 应根据具体 output 调整
5. **Wayland frame callback 帧同步**：替代 sleep 实现 vsync-aligned 渲染

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-24-wayland-pet-mode-layer-shell.md`. 

Two execution options:
1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration
2. **Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
