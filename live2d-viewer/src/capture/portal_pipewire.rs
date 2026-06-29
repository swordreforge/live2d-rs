use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use wayland_client::{
    protocol::{wl_output, wl_registry, wl_shm, wl_shm_pool},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use crate::capture::frame::{CapturedFrame, FrameSender};

macro_rules! noop {
    ($ty:ty) => {
        impl Dispatch<$ty, ()> for State {
            fn event(_: &mut Self, _: &$ty, _: <$ty as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    };
}

struct State {
    shm: Option<wl_shm::WlShm>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    outputs: Vec<wl_output::WlOutput>,
    frame: Option<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1>,
    pool: Option<wl_shm_pool::WlShmPool>,
    buf: Option<wayland_client::protocol::wl_buffer::WlBuffer>,
    pool_fd: Option<OwnedFd>,
    pool_size: usize,
    w: u32,
    h: u32,
    stride: u32,
    buffer_ready: bool,
    copy_done: bool,
    failed: bool,
    tx: FrameSender,
    stop: Arc<AtomicBool>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self, reg: &wl_registry::WlRegistry,
        event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _: &(), _: &Connection, qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match &interface[..] {
                "wl_shm" => s.shm = Some(reg.bind(name, 1, qh, ())),
                "zwlr_screencopy_manager_v1" if version >= 3 => s.screencopy = Some(reg.bind(name, 3, qh, ())),
                "wl_output" => s.outputs.push(reg.bind::<wl_output::WlOutput, _, _>(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

noop!(wl_shm::WlShm);
noop!(wl_output::WlOutput);
noop!(zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);
noop!(wl_shm_pool::WlShmPool);
noop!(wayland_client::protocol::wl_buffer::WlBuffer);

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        s: &mut Self, _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: <zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1 as wayland_client::Proxy>::Event,
        _: &(), _: &Connection, qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { width, height, stride, .. } => {
                s.w = width;
                s.h = height;
                s.stride = stride;
                let size = stride as usize * height as usize;
                s.pool.take();
                s.buf.take();
                if let Some(ref shm) = s.shm {
                    if let Ok(fd) = create_memfd(size) {
                        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
                        let buf = pool.create_buffer(0, width as i32, height as i32, stride as i32, wl_shm::Format::Xrgb8888, qh, ());
                        s.pool = Some(pool);
                        s.buf = Some(buf);
                        s.pool_size = size;
                        s.pool_fd = Some(fd);
                    }
                }
                s.buffer_ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => s.copy_done = true,
            zwlr_screencopy_frame_v1::Event::Failed { .. } => s.failed = true,
            _ => {}
        }
    }
}

fn create_memfd(size: usize) -> std::io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    let raw = unsafe { libc::memfd_create(b"sc\0".as_ptr().cast(), 0) };
    if raw < 0 { return Err(std::io::Error::last_os_error()); }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::ftruncate(fd.as_raw_fd(), size as _) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

fn mmap_read(fd: &OwnedFd, w: u32, h: u32, stride: u32) -> Vec<u8> {
    let size = stride as usize * h as usize;
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    unsafe {
        let ptr = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0);
        if ptr != libc::MAP_FAILED {
            let src = std::slice::from_raw_parts(ptr as *const u8, size);
            for y in 0..h as usize {
                let sr = y * stride as usize;
                let dr = y * w as usize * 4;
                for x in 0..w as usize {
                    let si = sr + x * 4;
                    let di = dr + x * 4;
                    rgba[di] = src[si + 2];
                    rgba[di + 1] = src[si + 1];
                    rgba[di + 2] = src[si];
                    rgba[di + 3] = 255;
                }
            }
            libc::munmap(ptr, size);
        }
    }
    rgba
}

pub fn run(tx: FrameSender, stop: Arc<AtomicBool>) -> Result<()> {
    let conn = Connection::connect_to_env().context("Wayland connect")?;
    let mut eq = conn.new_event_queue();
    let qh = eq.handle();
    conn.display().get_registry(&qh, ());

    let mut s = State {
        shm: None, screencopy: None, outputs: vec![],
        frame: None, pool: None, buf: None, pool_fd: None, pool_size: 0,
        w: 0, h: 0, stride: 0, buffer_ready: false, copy_done: false, failed: false,
        tx, stop,
    };

    eq.roundtrip(&mut s).context("roundtrip")?;

    let output = s.outputs.first().ok_or_else(|| anyhow::anyhow!("no outputs"))?.clone();
    let sc = s.screencopy.take().ok_or_else(|| anyhow::anyhow!("no screencopy"))?;

    log::info!("wlr-screencopy ready, capturing {}x{}...", s.w, s.h);

    loop {
        if s.stop.load(Ordering::Relaxed) { break; }

        s.buffer_ready = false;
        s.copy_done = false;
        s.failed = false;

        // 1. Request capture
        s.frame = Some(sc.capture_output(1, &output, &qh, ()));
        conn.flush().ok();

        // 2. Wait for buffer event (format/size/stride)
        while !s.buffer_ready && !s.failed {
            eq.blocking_dispatch(&mut s).context("wait buffer")?;
        }
        if s.failed { s.frame.take(); continue; }

        // 3. Copy to our buffer
        if let (Some(ref frame), Some(ref buf)) = (&s.frame, &s.buf) {
            frame.copy(buf);
        }
        conn.flush().ok();

        // 4. Wait for ready
        while !s.copy_done && !s.failed {
            eq.blocking_dispatch(&mut s).context("wait ready")?;
        }
        if s.failed { s.frame.take(); continue; }

        // 5. Read pixels
        if let Some(ref fd) = s.pool_fd {
            let rgba = mmap_read(fd, s.w, s.h, s.stride);
            log::debug!("captured frame {}x{} ({} bytes)", s.w, s.h, rgba.len());
            let _ = s.tx.send(CapturedFrame { data: rgba, width: s.w, height: s.h });
        }

        // 6. Destroy frame (start fresh next iteration)
        s.frame.take();
        conn.flush().ok();
        eq.roundtrip(&mut s).context("roundtrip after destroy")?;
    }

    log::info!("wlr-screencopy stopped");
    Ok(())
}
