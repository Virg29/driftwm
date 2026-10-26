//! Test-client side of the two screen-capture protocols, wlr-screencopy and
//! ext-image-copy-capture. The managers are bound through a registry of their
//! own, so a scenario opts in with [`Client::bind_capture_globals`] and every
//! other client stays as it was.

use std::os::fd::AsFd as _;
use std::os::unix::fs::FileExt as _;
use std::sync::{Arc, Mutex};

use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use super::client::{Client, State, shm_file};

#[derive(Default)]
pub struct CaptureGlobals {
    pub screencopy: Option<ZwlrScreencopyManagerV1>,
    pub output_sources: Option<ExtOutputImageCaptureSourceManagerV1>,
    pub image_copy: Option<ExtImageCopyCaptureManagerV1>,
}

pub type Globals = Arc<Mutex<CaptureGlobals>>;

/// What the compositor said about one wlr frame or ext session/frame.
#[derive(Default, Debug)]
pub struct CaptureRecord {
    /// wlr `buffer` as (width, height, stride), or ext `buffer_size` with a
    /// stride of zero.
    pub shm: Option<(i32, i32, i32)>,
    pub ready: bool,
    pub failed: bool,
    /// The ext frame's `failed` reason.
    pub failure_reason: Option<ext_image_copy_capture_frame_v1::FailureReason>,
}

pub type Record = Arc<Mutex<CaptureRecord>>;

/// A client-allocated shm buffer for a capture to land in. `file` backs the
/// whole pool, which can be larger than the buffer inside it.
pub struct CaptureBuffer {
    file: std::fs::File,
    _pool: WlShmPool,
    pub buffer: WlBuffer,
}

impl CaptureBuffer {
    /// Every byte of the pool, buffer and slack alike.
    pub fn pool_bytes(&self) -> Vec<u8> {
        let len = self.file.metadata().unwrap().len() as usize;
        let mut bytes = vec![0u8; len];
        self.file.read_exact_at(&mut bytes, 0).unwrap();
        bytes
    }
}

impl Client {
    /// Bind the capture managers. They arrive on the next roundtrip.
    pub fn bind_capture_globals(&mut self) -> Globals {
        let globals = Globals::default();
        let _registry = self.display.get_registry(&self.qh, globals.clone());
        self.flush();
        globals
    }

    fn output_named(&self, name: &str) -> WlOutput {
        self.state
            .outputs
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(output, _)| output.clone())
            .unwrap_or_else(|| panic!("no wl_output named {name}"))
    }

    /// Ask for a wlr-screencopy of the whole output, or of `region` (logical
    /// `x, y, w, h` inside it). The buffer parameters arrive on the next
    /// roundtrip.
    pub fn wlr_capture(
        &mut self,
        globals: &Globals,
        output: &str,
        region: Option<(i32, i32, i32, i32)>,
    ) -> (ZwlrScreencopyFrameV1, Record) {
        let output = self.output_named(output);
        let globals = globals.lock().unwrap();
        let manager = globals.screencopy.as_ref().expect("wlr-screencopy bound");
        let record = Record::default();
        let frame = match region {
            None => manager.capture_output(0, &output, &self.qh, record.clone()),
            Some((x, y, w, h)) => {
                manager.capture_output_region(0, &output, x, y, w, h, &self.qh, record.clone())
            }
        };
        (frame, record)
    }

    /// A `size` buffer of `format` at `offset` into a pool of `pool_len`
    /// bytes, its rows `stride` apart, every pool byte set to `fill` first so
    /// a test can tell what the compositor wrote.
    pub fn capture_buffer(
        &mut self,
        format: wl_shm::Format,
        size: (i32, i32),
        stride: i32,
        offset: i32,
        pool_len: usize,
        fill: u8,
    ) -> CaptureBuffer {
        let file = shm_file(&vec![fill; pool_len]);
        let shm = self.state.shm.as_ref().expect("wl_shm bound");
        let pool = shm.create_pool(file.as_fd(), pool_len as i32, &self.qh, ());
        let buffer = pool.create_buffer(offset, size.0, size.1, stride, format, &self.qh, ());
        CaptureBuffer {
            file,
            _pool: pool,
            buffer,
        }
    }

    /// An ext-image-copy-capture session on `output`, without cursors. Its
    /// buffer constraints arrive on the next roundtrip.
    pub fn ext_capture_session(
        &mut self,
        globals: &Globals,
        output: &str,
    ) -> (ExtImageCopyCaptureSessionV1, Record) {
        let output = self.output_named(output);
        let globals = globals.lock().unwrap();
        let sources = globals
            .output_sources
            .as_ref()
            .expect("ext output capture sources bound");
        let manager = globals
            .image_copy
            .as_ref()
            .expect("ext-image-copy-capture bound");
        let source = sources.create_source(&output, &self.qh, ());
        let record = Record::default();
        let session = manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &self.qh,
            record.clone(),
        );
        source.destroy();
        (session, record)
    }

    pub fn ext_capture(
        &mut self,
        session: &ExtImageCopyCaptureSessionV1,
        buffer: &WlBuffer,
    ) -> (ExtImageCopyCaptureFrameV1, Record) {
        let record = Record::default();
        let frame = session.create_frame(&self.qh, record.clone());
        frame.attach_buffer(buffer);
        frame.damage_buffer(0, 0, i32::MAX, i32::MAX);
        frame.capture();
        (frame, record)
    }
}

impl Dispatch<WlRegistry, Globals> for State {
    fn event(
        _state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        globals: &Globals,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        let mut globals = globals.lock().unwrap();
        if interface == ZwlrScreencopyManagerV1::interface().name {
            let version = version.min(ZwlrScreencopyManagerV1::interface().version);
            globals.screencopy = Some(registry.bind(name, version, qh, ()));
        } else if interface == ExtOutputImageCaptureSourceManagerV1::interface().name {
            let version = version.min(ExtOutputImageCaptureSourceManagerV1::interface().version);
            globals.output_sources = Some(registry.bind(name, version, qh, ()));
        } else if interface == ExtImageCopyCaptureManagerV1::interface().name {
            let version = version.min(ExtImageCopyCaptureManagerV1::interface().version);
            globals.image_copy = Some(registry.bind(name, version, qh, ()));
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, Record> for State {
    fn event(
        _state: &mut Self,
        _frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        record: &Record,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut record = record.lock().unwrap();
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(wl_shm::Format::Xrgb8888),
                width,
                height,
                stride,
            } => record.shm = Some((width as i32, height as i32, stride as i32)),
            zwlr_screencopy_frame_v1::Event::Ready { .. } => record.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => record.failed = true,
            _ => (),
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, Record> for State {
    fn event(
        _state: &mut Self,
        _session: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        record: &Record,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut record = record.lock().unwrap();
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                record.shm = Some((width as i32, height as i32, 0));
            }
            ext_image_copy_capture_session_v1::Event::Stopped => record.failed = true,
            _ => (),
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, Record> for State {
    fn event(
        _state: &mut Self,
        _frame: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        record: &Record,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut record = record.lock().unwrap();
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => record.ready = true,
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                record.failed = true;
                record.failure_reason = reason.into_result().ok();
            }
            _ => (),
        }
    }
}

macro_rules! no_events {
    ($($proxy:ty),*) => {$(
        impl Dispatch<$proxy, ()> for State {
            fn event(
                _state: &mut Self,
                _proxy: &$proxy,
                _event: <$proxy as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

no_events!(
    ZwlrScreencopyManagerV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCaptureSourceV1,
    ExtImageCopyCaptureManagerV1
);
