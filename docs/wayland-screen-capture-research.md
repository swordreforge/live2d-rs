# Wayland 屏幕录制系统 — 研究提纲

> 视觉识别的前置管道：在 wayland 上捕获屏幕帧，后续接入 AI 视觉系统。
> MVP 目标：跑通屏幕录制，拿到每帧 RGB 数据。

---

## 1. 总体架构

```
┌─────────────────────────────────────────────────────┐
│                  live2d-viewer                        │
│                                                       │
│  ┌──────────────────────────────────────────┐         │
│  │          winit event loop (主线程)        │         │
│  │  ┌────────┐ ┌─────────┐ ┌────────────┐  │         │
│  │  │ Live2D │ │ egui UI │ │ AppState   │  │         │
│  │  │ 渲染    │ │ 面板    │ │ 状态管理    │  │         │
│  │  └────────┘ └─────────┘ └────────────┘  │         │
│  └──────────────────────────────────────────┘         │
│                                                       │
│  ┌──────────────────────────────────────────┐         │
│  │      Screen Capture 线程 (新)             │         │
│  │                                           │         │
│  │  ┌──────────┐  ┌───────────┐  ┌──────┐  │         │
│  │  │ ashpd    │─▶│ PipeWire  │─▶│ 帧    │  │         │
│  │  │ (D-Bus)  │  │ (stream)  │  │ 队列   │  │         │
│  │  └──────────┘  └───────────┘  └──────┘  │         │
│  └──────────────────────────────────────────┘         │
│                            │                           │
│                            ▼ RGB 帧数据                │
│                     ┌──────────────┐                  │
│                     │ 视觉识别模块    │ (后续)          │
│                     │ (AI visual)  │                  │
│                     └──────────────┘                  │
└─────────────────────────────────────────────────────┘
```

**现有模板参考**：`wayland_pet.rs` 的独立线程模式 — 一个线程跑自己的事件循环，通过 `mpsc` channel 与主线程通信。Screen capture 可以复用同样的模式。

---

## 2. 两条技术路线

### 路线 A：xdg-desktop-portal + PipeWire（推荐，通用路线）

| 组件 | Rust crate | 版本 |
|---|---|---|
| Portal D-Bus 客户端 | `ashpd` | 0.13.12 (feature: `screencast`) |
| PipeWire 管道 | `pipewire` | 0.10.0 (原 `pipewire-rs`) |
| SPA 参数/格式 | `libspa` | 0.10.0 |
| 异步运行时 | `tokio`（已有依赖） | 1.x |
| DMA-BUF 格式常量 | `drm-fourcc` | 2.2.0 |

**工作流**（4 步 D-Bus + 连接 PipeWire）:

```
1. ashpd::desktop::screencast::Screencast::new()
   → 创建 portal session

2. select_sources()
   → 选择源：Monitor / Window，cursor mode

3. start() → 弹出系统权限对话框（用户点"分享"）
   → 返回 streams: Vec<(node_id, properties)>

4. open_pipe_wire_remote() → 返回 OwnedFd
   → 连接 PipeWire: context.connect(Some(fd))
   → 创建 pw_stream，设置 TARGET_OBJECT = node_id
   → 自动链接到 compositor 的 producer node
   → 帧回调: stream.dequeue_buffer()
```

**优点**：通用（GNOME / KDE / wlroots / Hyprland 全兼容）。\
**缺点**：每次启动要用户点权限对话框（可加 `persist_mode` + `restore_token` 缓解）。

### 路线 B：wlr-screencopy 直接捕获（wlroots 专用，更简单）

| 组件 | Rust crate | 版本 |
|---|---|---|
| Wayland 客户端 | `wayland-client`（已有） | 0.31.x（随 sctk 引入） |
| wlr-screencopy 协议 | `wayland-protocols-wlr` | 0.3.12 |
| DMA-BUF | 复用已存在的 `gbm` 绑定 | — |

**工作流**：

```
1. 从 registry bind zwlr_screencopy_manager_v1
2. capture_output(overlay_cursor, output)
3. 监听 frame.buffer() / .linux_dmabuf() / .buffer_done() 回调
4. copy() 到 wl_buffer（SHM）或等待 dma-buf ready
5. mmap 读取像素 / glEGLImageTargetTexture2DOES 导入 GL
```

**优点**：无权限对话框，零依赖除 wayland-client，完全自控。\
**缺点**：仅 wlroots 系（Sway / Hyprland / River）。Hyprland 最近切到 `ext-image-copy-capture-v1`，
`wlr-screencopy-unstable-v1` 已被标记为 deprecated。

### 路线 C（推荐给新项目）：`ext-image-copy-capture-v1`

wlr-screencopy 的现代替代，wayland-protocols staging 中。
wl-mirror 已切过去。Hyprland 2026 年 2 月已合并支持。
wayland-protocols 中尚未有正式 Rust 绑定发布，但可以直接用 wayland-scanner 生成。

**MVP 建议先选路线 A**（通用 + 成熟 crate 生态），后续需要更低延迟再考虑路线 C。

---

## 3. 帧管线（MVP 需要的最小实现）

### 3.1 格式协商

PipeWire stream 创建时声明支持的格式：

```rust
let stream = pw::stream::Stream::new(&core, "screen-capture",
    properties! {
        *pw::keys::MEDIA_TYPE     => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE     => "Screen",
        *pw::keys::TARGET_OBJECT  => node_id.to_string(),
    },
)?;

// → 声明接受 BGRA/RGBA/NV12 等 raw 格式
```

Compositor 回复 `param_changed` 事件告知选定的格式、分辨率、帧率。

### 3.2 MVP 帧读取（CPU 路径）

```rust
.process(|stream, _| {
    if let Some(buf) = stream.dequeue_buffer() {
        let spa_buf = buf.buffer();               // *const spa_buffer
        let plane   = unsafe { &*spa_buf }.datas[0]; // DMA-BUF or SHM
        let chunk   = unsafe { &*plane.chunk };
        // SHM: plane.data → &[u8] (CPU 可直接读)
        // DMA-BUF: plane.fd → EGL/GL 导入 → readPixels 回 CPU
        // 存入环形缓冲区 → 通知主线程
    }
})
```

### 3.3 帧队列设计

```
mpsc::Sender<CapturedFrame> ———▶ mpsc::Receiver<CapturedFrame>
   capture 线程发                   主线程收（在 winit 帧循环内）

CapturedFrame {
    data: Vec<u8>,        // RGBA 像素
    width: u32,
    height: u32,
    stride: u32,
    timestamp: Instant,
}
```

MVP 不需要零拷贝，`Vec<u8>` 传输即可。后续优化：
- DMA-BUF 零拷贝直接 GL 纹理
- 跨线程 `Arc<[u8]>` 避免拷贝
- 环形缓冲区控制内存

### 3.4 帧率控制

PipeWire 的流按 compositor 刷新率推送（通常 60fps）。
MVP：接收所有帧，最多以 10-15fps 向主线程投递（降低视觉识别系统的帧率）。
后续：自适应帧率，只在检测到画面变化时投递。

---

## 4. 文件组织（新）

```
live2d-viewer/src/
├── capture/
│   ├── mod.rs              # CaptureSession trait、公开接口
│   ├── portal_pipewire.rs  # 路线 A：ashpd + pipewire 实现
│   ├── frame.rs            # CapturedFrame 类型、帧队列
│   ├── shm.rs              # SHM 帧读取（CPU 路径）
│   └── dmabuf.rs           # DMA-BUF 导入 GL 纹理（后续）
├── vision/                 # （后续阶段）
│   ├── mod.rs
│   └── recognizer.rs       # AI 视觉识别接口
```

---

## 5. MVP 实现步骤

| 步骤 | 内容 | 时间估计 |
|---|---|---|
| 1 | 添加 `ashpd`、`pipewire`、`libspa` 依赖到 `live2d-viewer/Cargo.toml`，feature gate 为 `capture` | 15 分钟 |
| 2 | 实现 `capture/frame.rs`：`CapturedFrame` 结构体 + 跨线程帧通道 | 30 分钟 |
| 3 | 实现 `capture/portal_pipewire.rs`：4 步 portal 流程 + PipeWire stream 连接 + 帧回调读 CPU 数据 | 2 小时 |
| 4 | 实现 `capture/mod.rs`：封装启动/停止接口，暴露 `start_capture()` / `stop_capture()` | 30 分钟 |
| 5 | 主线程集成：初始化时接上 capture 通道，帧到达时输出日志/统计（验证通路） | 30 分钟 |
| 6 | 命令行参数：`--capture` 或设置面板开关 | 20 分钟 |
| 7 | 测试：在 Sway/Hyprland/GNOME 上验证能收到帧数据 | 调试时间 |

**MVP 交付**：`cargo run --release -p live2d-viewer -- --capture` → 终端打印每帧的尺寸和序号。

---

## 6. Feature gate & Cargo.toml 改动

```toml
[features]
default = ["static-link", "ai"]
capture = ["ashpd/screencast", "pipewire", "libspa"]   # 新增

[dependencies]
# ------- 新增 capture 依赖 -------
ashpd = { version = "0.13", optional = true, features = [
    "screencast",
] }
pipewire = { version = "0.10", optional = true }
libspa = { version = "0.10", optional = true }
# ------- 已有 -------
tokio = { version = "1", default-features = false, features = ["rt"] }
```

为什么 optional：大多数用户不需要 capture 功能，默认不编译 PipeWire 依赖。

---

## 7. 跟现有代码的协作

- **主线程集成**：在主 winit 事件循环中插入 capture 帧消费。在 `main.rs` 的 `Event::WindowEvent { RedrawRequested, .. }` 块内 drain capture channel。
- **Wayland 检测**：已有 `on_wayland` 布尔标志（`main.rs:44`）→ capture 只在 Wayland 上启用。
- **X11 fallback**：X11 可以用 `xcb` → `XGetImage` / `XShmGetImage` 做 screen capture，但不在 MVP 范围内。
- **Pet mode 共存**：capture 线程和 wayland_pet 线程各自独立，不冲突。

---

## 8. 后续可能的扩展

| 阶段 | 功能 | 备注 |
|---|---|---|
| MVP | 帧捕获到 `Vec<u8>` RGB 数据 | 记录帧尺寸、序号 |
| Phase 2 | DMA-BUF 零拷贝导入 GL 纹理 | 通过 `eglCreateImageKHR` + `glEGLImageTargetTexture2DOES` |
| Phase 3 | 帧率控制 + 差异检测（不传重复帧） | 降低后续 AI 推理压力 |
| Phase 4 | 视觉识别 AI 接口 | 从 `CapturedFrame` 提取特征 |
| Phase 5 | 多源选择（窗口 / 区域截图） | portal 的 `SourceType::Window` |
| Phase 6 | X11 支持 | `XShmGetImage` |

---

## 9. 关键参考链接

| 主题 | 链接 |
|---|---|
| ashpd 文档 | https://docs.rs/ashpd/latest/ashpd/desktop/screencast/index.html |
| ashpd screencast 示例 | https://github.com/bilelmoussaoui/ashpd/blob/main/examples/screen_cast_pw.rs |
| pipewire-rs API | https://pipewire.pages.freedesktop.org/pipewire-rs/pipewire/ |
| PipeWire 教程 5 (C) | https://docs.pipewire.org/page_tutorial5.html |
| xdg-desktop-portal ScreenCast 规范 | https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html |
| wl-screenrec (Rust 参考实现) | https://github.com/russelltg/wl-screenrec |
| waycap-rs (完整方案) | https://crates.io/crates/waycap-rs |
| libscreencapture-wayland (C++ 参考) | https://github.com/DafabHoid/libscreencapture-wayland |
