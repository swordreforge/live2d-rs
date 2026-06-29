use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use pipewire as pw;

use crate::capture::frame::{CapturedFrame, FrameSender};

struct PortalSession {
    _session: ashpd::desktop::Session<Screencast>,
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
        let proxy = Screencast::new().await?;

        let session = proxy.create_session(Default::default()).await?;

        let source_types = SourceType::Monitor | SourceType::Window;
        let select_options = ashpd::desktop::screencast::SelectSourcesOptions::default()
            .set_cursor_mode(CursorMode::Embedded)
            .set_sources(source_types)
            .set_multiple(true)
            .set_persist_mode(ashpd::desktop::PersistMode::ExplicitlyRevoked);

        proxy
            .select_sources(&session, select_options)
            .await
            .context("portal select_sources failed (user may have cancelled)")?;

        log::info!("ScreenCast portal dialog should appear now");

        let streams = proxy
            .start(
                &session,
                None,
                ashpd::desktop::screencast::StartCastOptions::default(),
            )
            .await?;

        let response = streams.response()?;

        let pw_fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await?;

        let raw_fd = pw_fd.into_raw_fd();
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

        let node_id = response
            .streams()
            .first()
            .map(|s| s.pipe_wire_node_id())
            .ok_or_else(|| anyhow::anyhow!("portal returned no streams"))?;

        Ok(PortalSession {
            _session: session,
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

fn bgrx_to_rgba(bgrx: &[u8], rgba: &mut [u8]) {
    for (src, dst) in bgrx.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 255;
    }
}
