# Making a Wayland Window Invisible: A Case Study in Compositor Constraints

## Background

The live2d-rs project has a "pet mode" — a small always-on-top window with a Live2D character overlay. On X11, this works trivially: `window.set_visible(false)` hides the X11 toplevel while keeping it mapped. On Wayland, this call silently kills event delivery (tray icon operations stop working), because Wayland's security model doesn't allow clients to arbitrarily hide surfaces.

The desired behavior: when the user minimizes the AlwaysOnTop pet, the window should become invisible (the character remains visible on the compositor layer-shell surface), but the process must stay alive for tray icon interaction.

This article documents the iterative journey to achieve this on **niri**, a scrollable-tiling Wayland compositor, and the constraints uncovered along the way.

---

## Attempt 1: WINIT_UNIX_BACKEND (Failed — API Removed)

The first instinct was to force winit's backend to X11, where `set_visible(false)` works:

```rust
std::env::set_var("WINIT_UNIX_BACKEND", "x11");
```

**Problem**: This environment variable was removed in winit 0.29.0. The [winit changelog](https://github.com/rust-windowing/winit/blob/master/CHANGELOG.md#0290) explicitly marks it as removed. The variable is silently ignored, and the window always opens on the native Wayland backend on a Wayland-only compositor.

**Lesson**: Always verify API existence against the actual dependency version, not general knowledge. The code was dead code carrying no effect.

---

## Attempt 2: Raw X11 Protocol via x11rb (Abandoned — Too Invasive)

Since winit doesn't expose a Wayland `set_visible`, the next idea was to open an X11 connection alongside the existing window and use raw `x11rb` to `XUnmapWindow`:

```rust
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
```

**Problems**:
- Requires `with_x11()` on the winit window builder, which changes the windowing backend
- Introduces a `x11rb` dependency solely for this one operation
- The window already lives on Wayland — an X11 connection can't manipulate a Wayland surface
- On pure Wayland compositors (niri), there is no X11 display to connect to

**Lesson**: Mixed-backend approaches add complexity without solving the fundamental platform constraint. On Wayland, you own your surface — you can't ask another protocol to hide it.

---

## Attempt 3: Off-Screen Positioning (Failed — No-Op on Wayland)

On X11, you can move a window off-screen:

```rust
window.set_outer_position(Position::Logical(30000.0, 30000.0));
```

**Problem**: `set_outer_position` on the Wayland backend is a documented no-op. Wayland explicitly forbids clients from positioning their surfaces — the compositor owns placement. This is by design: Wayland's security model prevents windows from positioning themselves outside the visible area to avoid clickjacking and other attacks.

**Lesson**: Wayland's security model is fundamentally different from X11. Anything related to window position, stacking order, or visibility is compositor authority. Don't fight it — work with it.

---

## Attempt 4: Minimal Window Resize (1×1 — Rejected by Compositor)

The approach that works on KDE/GNOME: resize to 1×1 and set max size to 1×1:

```rust
window.set_max_inner_size(Some(LogicalSize::new(1.0, 1.0)));
let _ = window.request_inner_size(LogicalSize::new(1.0, 1.0));
// Force EGL surface resize
surface.resize(&gl_context, NonZeroU32::new(1).unwrap(), NonZeroU32::new(1).unwrap());
```

**What happened on niri**:

```
[pet] request_minimize: doing 1x1 resize
[pet/wayland] surface configured: 400x500
```

The compositor **rejected** the 1×1 request and set the surface to 400×500. Investigation: niri enforces a minimum surface size of ~50×50 logical pixels, typical of Wayland compositors that don't want zero-dimensional surfaces. The `surface configured` callback receives the compositor-chosen size.

**Lesson**: On Wayland, `request_inner_size` is a **request**, not a command. The compositor may (and often does) respond with a different size. The `Resized` event must handle this gracefully.

---

## The Egui Font Crash (Corner Case at Small Sizes)

When the 1×1 resize was rejected and the surface came back at 400×500, the next frame triggered:

```
thread 'main' panicked at epaint-0.27.2/src/text/font.rs:92:9:
assertion failed: scale_in_pixels > 0.0
```

**Root cause**: The egui frame processing ran with `minimized_to_float = true`. The font tessellation path calculated `scale_in_pixels = pixels_per_point * font_size`. Under certain state transitions (partial resize, pending state), `pixels_per_point` or the effective font size could resolve to 0.

The egui source has two guards:

```rust
// epaint/src/text/font.rs:92-93
assert!(scale_in_pixels > 0.0);
assert!(pixels_per_point > 0.0);
```

These are **debug-only development assertions**, not production error handling. On a compositor-enlarged surface with a partially applied minimized state, the egui context can compute a zero scale.

**Fix**: Skip the entire egui frame (begin/end/tessellate/paint) when the window is in the minimized+AlwaysOnTop state:

```rust
if minimized_to_float && pet_mode == AlwaysOnTop {
    // 1×1 surface, no egui frame — avoid font.rs assertion
} else {
    // normal egui frame processing
}
```

**Lesson**: When you disable one rendering path (the play button), check if other paths (egui frame) have hidden dependencies on window geometry. Also: `request_inner_size` is asynchronous on Wayland — there's a window where the old size is still active but the new rendering state is applied.

---

## The 0×0 Question

Could we use 0×0 instead of 1×1? No — and here's why:

**Layer 1 — `NonZeroU32`**: glutin's `surface.resize()` requires `NonZeroU32`. `NonZeroU32::new(0)` returns `None`, and our code skips the resize. The intent (minimize the surface) becomes a no-op.

**Layer 2 — Fractional sizes (< 1.0)**: `(0.5 * scale_factor) as u32` truncates to 0, same `NonZeroU32` problem. `.max(1)` becomes essential but defeats the purpose.

**Layer 3 — Resized guard loop**: With `float_logical = 0.0`, the guard computes `max_phys = 0`, and `size.width > 0` is **always true** for any real surface. This creates an infinite loop:
```
compositor: here's 400×500
client: request_inner_size(0, 0)!
compositor: no, here's 400×500
client: request_inner_size(0, 0)!
... infinite
```

With `float_logical = 1.0`, `max_phys = 1`, and the guard fires exactly once (or a few times) before the compositor settles.

**Lesson**: Small does not mean zero. Fractions round to zero and create edge cases. The `.max(1)` pattern is a safety net for all of them.

---

## The Final Solution: Hide in Place

The working approach doesn't actually "hide" the window. Instead, it renders nothing:

```
┌─────────────────────────────────────────────┐
│  request_minimize → 1×1 (niri → 400×500)    │
│  minimized_to_float = true                   │
│                                              │
│  clear_color = transparent (rgba 0,0,0,0)   │
│  model = skip (!minimized_to_float guard)    │
│  egui frame = skip (AlwaysOnTop guard)       │
│  play button = skip (AlwaysOnTop guard)      │
│  swap_buffers → nothing visible              │
└─────────────────────────────────────────────┘
```

Three rendering paths are gated independently, and all three must be closed for invisibility:

| Path | Guard | Rationale |
|---|---|---|
| Live2D model | `!minimized_to_float` | Pre-dates this work |
| egui UI | `minimized_to_float && AlwaysOnTop` | New — avoids font crash |
| Play button triangle | `minimized_to_float && AlwaysOnTop` | New — nothing to show |
| Clear color | `minimized_to_float && AlwaysOnTop` → transparent | New — no visual residue |

The 50×50 "float circle" path (for Windowed pet mode) keeps the play button and colored background. The AlwaysOnTop path gets transparent emptiness. Both use the same `minimized_to_float` infrastructure — only the rendering content differs.

---

## Key Takeaways for Wayland Window Management

1. **`request_inner_size` is advisory**, not authoritative. Always handle `WindowEvent::Resized` with the compositor's actual size.

2. **Wayland compositors enforce minimum surface sizes**. niri's is ~50px. Attempting below that results in the compositor overriding your size.

3. **Positioning is a compositor privilege**. `set_outer_position` is a no-op on Wayland. There is no Wayland equivalent of X11's `XMoveWindow` or `XUnmapWindow` — by design.

4. **`NonZeroU32` is your friend**. It documents the invariant "surface dimensions must be ≥ 1" in the type system. Use it instead of ad-hoc >0 checks.

5. **Rendering guards must be comprehensive**. Disabling one path (egui painter) while another still writes (model renderer) leaves surface contamination. Audit all paths when going invisible.

6. **Debug assertions in dependencies can surprise you**. egui's `assert!(scale_in_pixels > 0.0)` in font.rs is reasonable for normal operation, but doesn't anticipate a client using the context at 1×1 geometry. The fix is to avoid calling into the library at that geometry, not to patch the library.

7. **For pure Wayland environments, "hiding" a window is semantically impossible**. The best you can do is render nothing and hope the compositor cooperates. Wayland's design philosophy: surfaces are visible until destroyed. There is no mapped/unmapped state.

---

## References

- [winit 0.29 changelog — removed WINIT_UNIX_BACKEND](https://github.com/rust-windowing/winit/blob/master/CHANGELOG.md#0290)
- [egui epaint font.rs assertion](https://github.com/emilk/egui/blob/v0.27.2/crates/epaint/src/text/font.rs#L92)
- [Wayland protocol: shell surface (no hide operation)](https://wayland.app/protocols/wlr-layer-shell-unstable-v1)
- [niri compositor source](https://github.com/YaLTeR/niri)
