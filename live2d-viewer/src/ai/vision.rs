use std::sync::Mutex;

use base64::Engine;
use image::imageops::FilterType;

const MAX_DIMENSION: u32 = 512;
const JPEG_QUALITY: u8 = 70;

pub struct VisionSnapshot {
    pub base64: String,
    pub width: u32,
    pub height: u32,
}

static PENDING_SNAPSHOT: Mutex<Option<String>> = Mutex::new(None);

pub fn store_snapshot(base64: String) {
    *PENDING_SNAPSHOT.lock().unwrap() = Some(base64);
}

pub fn take_snapshot() -> Option<String> {
    PENDING_SNAPSHOT.lock().unwrap().take()
}

#[cfg(feature = "capture")]
pub fn store_frame_snapshot(frame: &crate::capture::CapturedFrame) {
    if let Some(snap) = encode_frame(frame) {
        store_snapshot(snap.base64);
    }
}

#[cfg(feature = "capture")]
pub fn encode_frame(frame: &crate::capture::CapturedFrame) -> Option<VisionSnapshot> {
    let rgba = image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())?;
    let (w, h) = if frame.width > frame.height {
        let scale = MAX_DIMENSION as f32 / frame.width as f32;
        ((MAX_DIMENSION), (frame.height as f32 * scale) as u32)
    } else {
        let scale = MAX_DIMENSION as f32 / frame.height as f32;
        ((frame.width as f32 * scale) as u32, (MAX_DIMENSION))
    };

    let scaled = image::DynamicImage::ImageRgba8(rgba).resize(w, h, FilterType::Lanczos3);

    let mut buf = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    encoder.encode_image(&scaled).ok()?;

    let base64 = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&buf)
    );

    Some(VisionSnapshot {
        base64,
        width: w,
        height: h,
    })
}
