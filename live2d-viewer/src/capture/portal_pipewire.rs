use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use pipewire as pw;

use crate::capture::frame::{CapturedFrame, FrameSender};

/// Holds the portal session alive for the entire capture duration.
struct PortalSession {
    _manager: lamco_portal::PortalManager,
    _handle: lamco_portal::PortalSessionHandle,
    fd: OwnedFd,
    node_id: u32,
}

pub fn run(tx: FrameSender, stop_flag: Arc<AtomicBool>) -> Result<()> {
    let portal = portal_handshake()?;
    log::info!(
        "Portal ready: node_id={}, fd={}",
        portal.node_id,
        portal.fd.as_raw_fd(),
    );
    pipewire_capture(tx, portal, stop_flag)
}

fn portal_handshake() -> Result<PortalSession> {
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

    rt.block_on(async {
        let manager = lamco_portal::PortalManager::with_default()
            .await
            .context("failed to create PortalManager")?;

        let (handle, _restore_token) = manager
            .create_session("live2d-capture".to_string(), None)
            .await
            .context("failed to create portal session")?;

        let raw_fd = handle.pipewire_fd();
        // Safety: the raw fd is owned by the session handle which lives on,
        // so the fd is valid for the entire session lifetime.
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let node_id = handle
            .streams()
            .last()
            .map(|s| s.node_id)
            .ok_or_else(|| anyhow::anyhow!("portal returned no streams"))?;

        Ok(PortalSession {
            _manager: manager,
            _handle: handle,
            fd,
            node_id,
        })
    })
}

fn pipewire_capture(
    tx: FrameSender,
    portal: PortalSession,
    stop_flag: Arc<AtomicBool>,
) -> Result<()> {
    pw::init();

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire mainloop")?;
    let context = pw::context::ContextBox::new(mainloop.loop_(), None)
        .context("failed to create PipeWire context")?;

    let core = context
        .connect_fd(portal.fd, None)
        .context("failed to connect PipeWire core via portal fd")?;

    let mut stream_props = pw::properties::PropertiesBox::new();
    stream_props.insert("node.name", "live2d-capture-input");

    let stream = pw::stream::StreamBox::new(&core, "live2d-capture", stream_props)
        .context("failed to create PipeWire stream")?;

    let _listener = stream
        .add_local_listener_with_user_data(tx)
        .process(move |s, user_tx| {
            if let Some(mut buf) = s.dequeue_buffer() {
                if let Some(data) = buf.datas_mut().first_mut() {
                    let size = data.chunk().size() as usize;
                    let offset = data.chunk().offset() as usize;
                    if size > 0 {
                        if let Some(slice) = data.data() {
                            // slice is the full maxsize buffer; frame data starts at offset
                            if offset + size <= slice.len() {
                                let frame_bytes = &slice[offset..offset + size];
                                let n_pixels = frame_bytes.len() / 4;
                                let mut rgba = vec![0u8; n_pixels * 4];
                                bgrx_to_rgba(frame_bytes, &mut rgba);
                                let frame = CapturedFrame {
                                    data: rgba,
                                    width: 0,
                                    height: 0,
                                };
                                let _ = user_tx.send(frame);
                            }
                        }
                    }
                }
            }
        })
        .register();

    let timer = {
        let stop = stop_flag.clone();
        let ml = mainloop.clone();
        mainloop.loop_().add_timer(move |_expirations| {
            if stop.load(Ordering::Relaxed) {
                ml.quit();
            }
        })
    };
    timer
        .update_timer(
            Some(Duration::from_millis(200)),
            Some(Duration::from_millis(200)),
        )
        .into_result()
        .context("failed to update timer")?;

    log::info!("PipeWire capture loop starting");
    mainloop.run();
    log::info!("PipeWire capture loop exited");
    Ok(())
}

/// Convert BGRx (32 bpp, B = byte 0, G = byte 1, R = byte 2, x = byte 3)
/// to RGBA (R = byte 0, G = byte 1, B = byte 2, A = 255).
fn bgrx_to_rgba(bgrx: &[u8], rgba: &mut [u8]) {
    for (src, dst) in bgrx.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 255;
    }
}
