//! A layer surface that resizes commits its new size before a buffer of that
//! size. Until it acks the configure, its old buffer is drawn against the edges
//! it is anchored to, not at the corner of the rect arranged for the new size.

use smithay::output::Output;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::shell::wlr_layer::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

use super::{Fixture, client, map_top_layer};

/// Where the output's only Top layer is drawn, output-local.
fn drawn(f: &mut Fixture, output: &Output) -> Rectangle<i32, Logical> {
    let layers = f.state().layers_on_sorted(output, Layer::Top);
    assert_eq!(layers.len(), 1);
    layers[0].1
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

#[test]
fn a_growing_bottom_layer_keeps_its_old_buffer_on_the_bottom_edge() {
    let mut f = Fixture::new();
    let output = f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let surface = map_top_layer(&mut f, id, "island", (300, 100), Some(Anchor::Bottom));
    assert_eq!(
        drawn(&mut f, &output),
        rect(810, 980, 300, 100),
        "precondition"
    );

    // Asking for more room, still showing the old buffer.
    let layer = f.client(id).layer(&surface);
    layer.set_configure_props(client::LayerConfigureProps {
        size: Some((300, 400)),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);
    assert_eq!(
        drawn(&mut f, &output),
        rect(810, 980, 300, 100),
        "the old buffer stays on the bottom edge"
    );

    // The buffer at the new size arrives.
    let layer = f.client(id).layer(&surface);
    layer.set_size(300, 400);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(drawn(&mut f, &output), rect(810, 680, 300, 400));
}

/// Grow the surface to `size` and settle the client, its buffer left as it was.
fn request_size(
    f: &mut Fixture,
    id: client::ClientId,
    surface: &wayland_client::protocol::wl_surface::WlSurface,
    size: (u32, u32),
) {
    let layer = f.client(id).layer(surface);
    layer.set_configure_props(client::LayerConfigureProps {
        size: Some(size),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);
}

#[test]
fn a_growing_layer_anchored_to_neither_vertical_edge_stays_centred() {
    let mut f = Fixture::new();
    let output = f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let surface = map_top_layer(&mut f, id, "island", (300, 100), None);
    assert_eq!(
        drawn(&mut f, &output),
        rect(810, 490, 300, 100),
        "precondition"
    );

    request_size(&mut f, id, &surface, (300, 400));
    assert_eq!(drawn(&mut f, &output), rect(810, 490, 300, 100));
}

#[test]
fn a_growing_top_layer_stays_on_the_top_edge() {
    let mut f = Fixture::new();
    let output = f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let surface = map_top_layer(&mut f, id, "island", (300, 100), Some(Anchor::Top));
    assert_eq!(
        drawn(&mut f, &output),
        rect(810, 0, 300, 100),
        "precondition"
    );

    request_size(&mut f, id, &surface, (300, 400));
    assert_eq!(drawn(&mut f, &output), rect(810, 0, 300, 100));
}

/// The protocol lets a client pick a buffer smaller than its configure; once it
/// has acked, that is where it means to be, and it is drawn where arranged.
#[test]
fn a_settled_layer_smaller_than_its_configure_is_drawn_where_arranged() {
    let mut f = Fixture::new();
    let output = f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let created = f
        .client(id)
        .create_layer(None, zwlr_layer_shell_v1::Layer::Top, "island");
    let surface = created.surface.clone();
    created.set_configure_props(client::LayerConfigureProps {
        size: Some((300, 400)),
        anchor: Some(Anchor::Bottom),
        exclusive_zone: Some(0),
        ..Default::default()
    });
    created.commit();
    f.roundtrip(id);
    let layer = f.client(id).layer(&surface);
    layer.set_size(300, 100);
    layer.attach_new_buffer();
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    assert_eq!(drawn(&mut f, &output), rect(810, 680, 300, 100));
}
