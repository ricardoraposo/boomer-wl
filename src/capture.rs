use smithay_client_toolkit::shm::{
    Shm,
    slot::{Buffer, SlotPool},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_output::WlOutput, wl_shm::Format},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::render::Image;

#[derive(Debug, Clone, Copy)]
struct ShmLayout {
    format: Format,
    width: u32,
    height: u32,
    stride: u32,
}

enum Status {
    Pending,
    Copying {
        pool: SlotPool,
        buffer: Buffer,
        layout: ShmLayout,
    },
    Ready(Image),
    Failed(String),
}

pub struct Capture {
    pub output: WlOutput,
    frame: ZwlrScreencopyFrameV1,
    layout: Option<ShmLayout>,
    y_invert: bool,
    status: Status,
}

pub struct Captures {
    entries: Vec<Capture>,
}

impl Captures {
    pub fn start<D>(
        manager: &ZwlrScreencopyManagerV1,
        outputs: impl IntoIterator<Item = WlOutput>,
        qh: &QueueHandle<D>,
    ) -> Self
    where
        D: Dispatch<ZwlrScreencopyFrameV1, usize> + 'static,
    {
        let entries = outputs
            .into_iter()
            .enumerate()
            .map(|(index, output)| Capture {
                frame: manager.capture_output(0, &output, qh, index),
                output,
                layout: None,
                y_invert: false,
                status: Status::Pending,
            })
            .collect();
        Self { entries }
    }

    pub fn is_finished(&self) -> bool {
        self.entries
            .iter()
            .all(|c| matches!(c.status, Status::Ready(_) | Status::Failed(_)))
    }

    pub fn into_images(self) -> Result<Vec<(WlOutput, Image)>, String> {
        let mut images = Vec::new();
        let mut errors = Vec::new();
        for capture in self.entries {
            capture.frame.destroy();
            match capture.status {
                Status::Ready(image) => images.push((capture.output, image)),
                Status::Failed(reason) => errors.push(reason),
                Status::Pending | Status::Copying { .. } => errors.push("capture did not finish".into()),
            }
        }
        if images.is_empty() {
            return Err(format!("could not capture any output: {}", errors.join("; ")));
        }
        Ok(images)
    }

    pub fn handle_event(
        &mut self,
        shm: &Shm,
        index: usize,
        event: zwlr_screencopy_frame_v1::Event,
        frame_version: u32,
    ) {
        let Some(capture) = self.entries.get_mut(index) else {
            return;
        };
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if let WEnum::Value(format) = format
                    && capture.layout.is_none()
                    && to_argb(format).is_some()
                {
                    capture.layout = Some(ShmLayout {
                        format,
                        width,
                        height,
                        stride,
                    });
                }
                if frame_version < 3 {
                    begin_copy(shm, capture);
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => begin_copy(shm, capture),
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => capture.y_invert = flags.contains(Flags::YInvert),
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                let status = std::mem::replace(&mut capture.status, Status::Pending);
                capture.status = match status {
                    Status::Copying {
                        mut pool,
                        buffer,
                        layout,
                    } => match buffer.canvas(&mut pool) {
                        Some(bytes) => Status::Ready(decode(bytes, layout, capture.y_invert)),
                        None => Status::Failed("screencopy buffer is still busy".into()),
                    },
                    _ => Status::Failed("compositor sent `ready` before a copy started".into()),
                };
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                capture.status = Status::Failed("compositor refused to copy the output".into());
            }
            _ => {}
        }
    }
}

fn begin_copy(shm: &Shm, capture: &mut Capture) {
    if !matches!(capture.status, Status::Pending) {
        return;
    }
    let Some(layout) = capture.layout else {
        capture.status = Status::Failed("compositor offered no supported shm format".into());
        return;
    };
    let size = layout.stride as usize * layout.height as usize;
    let result = SlotPool::new(size, shm)
        .map_err(|e| e.to_string())
        .and_then(|mut pool| {
            let (buffer, _) = pool
                .create_buffer(
                    layout.width as i32,
                    layout.height as i32,
                    layout.stride as i32,
                    layout.format,
                )
                .map_err(|e| e.to_string())?;
            Ok((pool, buffer))
        });
    capture.status = match result {
        Ok((pool, buffer)) => {
            capture.frame.copy(buffer.wl_buffer());
            Status::Copying { pool, buffer, layout }
        }
        Err(reason) => Status::Failed(format!("could not allocate screencopy buffer: {reason}")),
    };
}

fn to_argb(format: Format) -> Option<fn(u32) -> u32> {
    match format {
        Format::Argb8888 | Format::Xrgb8888 => Some(|p| p | 0xFF000000),
        Format::Abgr8888 | Format::Xbgr8888 => {
            Some(|p| 0xFF000000 | ((p & 0xFF) << 16) | (p & 0xFF00) | ((p >> 16) & 0xFF))
        }
        _ => None,
    }
}

fn decode(bytes: &[u8], layout: ShmLayout, y_invert: bool) -> Image {
    let convert = to_argb(layout.format).expect("layout only holds supported formats");
    let width = layout.width as usize;
    let height = layout.height as usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        let source_y = if y_invert { height - 1 - y } else { y };
        let row = &bytes[source_y * layout.stride as usize..][..width * 4];
        pixels.extend(
            row.chunks_exact(4)
                .map(|c| convert(u32::from_le_bytes([c[0], c[1], c[2], c[3]]))),
        );
    }
    Image {
        width: layout.width,
        height: layout.height,
        pixels,
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for crate::App {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(captures) = state.captures.as_mut() {
            captures.handle_event(&state.shm, *index, event, wayland_client::Proxy::version(frame));
        }
    }
}

wayland_client::delegate_noop!(crate::App: ignore ZwlrScreencopyManagerV1);
