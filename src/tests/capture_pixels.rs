//! Screen capture through the wire protocols, rendered by a real GL context:
//! a wlr-screencopy region shows that region of the output.
//!
//! `#[ignore]`d out of the default lane like the other pixel scenarios (needs
//! Mesa's surfaceless EGL, self-skipping when that is missing); each scenario
//! takes [`gl::lock`] first.

use smithay::output::Output;
use wayland_client::protocol::wl_shm::Format;
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1;

use super::capture_client::{CaptureBuffer, Record};
use super::client::ClientId;
use super::{Fixture, gl, map_window_shm};

const OUTPUT: (u16, u16) = (640, 400);
/// Inside the window, away from the output's origin.
const REGION: (i32, i32, i32, i32) = (100, 80, 200, 120);
/// Filled into every pool byte before a capture, to tell what it wrote.
const FILL: u8 = 0xAB;

const CONFIG: &str = "[background]\n\
                      type = \"none\"\n\
                      \n\
                      [decorations]\n\
                      default_mode = \"client\"\n\
                      shadow = false\n";

/// A window whose every texel differs from its neighbours, so two captures of
/// different parts of it can't agree by accident.
fn map_gradient_window(f: &mut Fixture, client: ClientId) {
    let (w, h) = (600, 360);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            pixels.extend_from_slice(&[x as u8, y as u8, ((x >> 8) | ((y >> 8) << 4)) as u8, 255]);
        }
    }
    map_window_shm(f, client, "gradient", (w as u16, h as u16), (w, h), &pixels);
}

fn setup(test: &str) -> Option<(Fixture, Output, ClientId)> {
    let renderer = gl::surfaceless_renderer(test)?;
    let mut f = Fixture::with_config(super::config(CONFIG));
    let output = f.add_output(1, OUTPUT);
    gl::install(&mut f, renderer);
    let client = f.add_client();
    map_gradient_window(&mut f, client);
    super::tick_until_settled(&mut f);
    Some((f, output, client))
}

fn teardown(mut f: Fixture, output: &Output) {
    // A rendered frame leaves per-output GPU state the teardown baseline knows
    // nothing about; drop it the way an output disconnect does.
    f.state().render.remove_output(&output.name());
    gl::uninstall(&mut f);
}

/// Render one frame of `output` and fulfil the captures queued on it, as both
/// backends do right after drawing.
fn render_captures(f: &mut Fixture, output: &Output) {
    let mut backend = f
        .state()
        .backend
        .take()
        .expect("the fixture has a renderer");
    let renderer = backend.renderer();
    let elements = crate::render::compose_frame(f.state(), renderer, output, Vec::new());
    crate::render::render_screencopy(f.state(), renderer, output, &elements);
    crate::render::render_capture_frames(f.state(), renderer, output, &elements);
    f.state().backend = Some(backend);
}

/// The buffer size and stride a wlr frame asked for.
fn wlr_params(record: &Record) -> (i32, i32, i32) {
    record
        .lock()
        .unwrap()
        .shm
        .expect("the frame announced its shm buffer")
}

/// Copy a wlr frame into the buffer it asked for, placed `offset` bytes into a
/// pool with `tail` spare bytes after it.
fn wlr_copy(
    f: &mut Fixture,
    client: ClientId,
    frame: &ZwlrScreencopyFrameV1,
    record: &Record,
    offset: i32,
    tail: usize,
) -> CaptureBuffer {
    let (w, h, stride) = wlr_params(record);
    let pool_len = offset as usize + (stride * h) as usize + tail;
    let buffer =
        f.client(client)
            .capture_buffer(Format::Xrgb8888, (w, h), stride, offset, pool_len, FILL);
    frame.copy(&buffer.buffer);
    buffer
}

/// RGB of each pixel of a `w`x`h` little-endian xRGB/ARGB image inside `bytes`,
/// row by row from `offset`, rows `stride` apart.
fn rgb_pixels(bytes: &[u8], offset: usize, stride: usize, (w, h): (i32, i32)) -> Vec<[u8; 3]> {
    let mut px = Vec::with_capacity((w * h) as usize);
    for row in 0..h as usize {
        let start = offset + row * stride;
        for p in bytes[start..start + w as usize * 4].chunks_exact(4) {
            px.push([p[2], p[1], p[0]]);
        }
    }
    px
}

/// The `(x, y, w, h)` rectangle of a tightly packed `full_w`-wide image.
fn crop(full: &[[u8; 3]], full_w: i32, (x, y, w, h): (i32, i32, i32, i32)) -> Vec<[u8; 3]> {
    let mut px = Vec::with_capacity((w * h) as usize);
    for row in y..y + h {
        let start = (row * full_w + x) as usize;
        px.extend_from_slice(&full[start..start + w as usize]);
    }
    px
}

#[test]
#[ignore = "needs Mesa surfaceless EGL; run with --include-ignored"]
fn a_wlr_region_capture_shows_that_region_of_the_output() {
    let _gl = gl::lock();
    let Some((mut f, output, client)) = setup("wlr region capture") else {
        return;
    };
    let name = output.name();

    let globals = f.client(client).bind_capture_globals();
    f.roundtrip(client);
    let (full, full_record) = f.client(client).wlr_capture(&globals, &name, None);
    let (region, region_record) = f.client(client).wlr_capture(&globals, &name, Some(REGION));
    f.roundtrip(client);
    assert_eq!(
        wlr_params(&region_record),
        (REGION.2, REGION.3, REGION.2 * 4),
        "a scale-1 region asks for a buffer of its own size"
    );
    let full_buffer = wlr_copy(&mut f, client, &full, &full_record, 0, 0);
    let region_buffer = wlr_copy(&mut f, client, &region, &region_record, 0, 0);
    f.roundtrip(client);
    render_captures(&mut f, &output);
    f.roundtrip(client);
    assert!(
        full_record.lock().unwrap().ready,
        "the full capture completed"
    );
    assert!(
        region_record.lock().unwrap().ready,
        "the region capture completed"
    );

    let out = (i32::from(OUTPUT.0), i32::from(OUTPUT.1));
    let full_px = rgb_pixels(&full_buffer.pool_bytes(), 0, out.0 as usize * 4, out);
    let region_px = rgb_pixels(
        &region_buffer.pool_bytes(),
        0,
        REGION.2 as usize * 4,
        (REGION.2, REGION.3),
    );
    let at_origin = crop(&full_px, out.0, (0, 0, REGION.2, REGION.3));
    assert_ne!(
        crop(&full_px, out.0, REGION),
        at_origin,
        "the scenario needs the region to look different from the output's top-left"
    );
    assert!(
        region_px == crop(&full_px, out.0, REGION),
        "the region capture holds the output's pixels at the region"
    );

    teardown(f, &output);
}
