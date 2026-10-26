//! Keyboard snapping: `move-to-snap` walks the focused element to the stops
//! drag snapping offers, `resize-to-snap` walks its right or bottom edge
//! between them and the usable area's edge — clients and stand-ins alike —
//! and neither ever lands the element on another window. A move also steps
//! out a gap off an adjacent window's side.

use smithay::desktop::Window;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Size};
use wayland_client::protocol::wl_surface::WlSurface as ClientSurface;

use super::client::ClientId;
use super::real::TempDir;
use super::{
    Fixture, MAX_TICKS, TICK, ack_but_keep_size, ack_with_size, config, configure_count, give_ssd,
    last_configured, map_window, map_window_with_limits, settle, window_by_app_id, window_position,
};
use crate::state::StageWindow;
use driftwm::config::{Action, Direction};

/// Park the viewport at `camera` and zoom 1 with nothing in flight, so a
/// camera target afterwards is one an action armed.
fn park_view(f: &mut Fixture, camera: Point<f64, Logical>) {
    f.state().with_output_state(|os| {
        os.camera = camera;
        os.camera_target = None;
        os.zoom = 1.0;
        os.zoom_target = None;
        os.zoom_animation_anchor = None;
    });
}

/// Map a client window of `size` and park it at `loc`.
fn window_at(
    f: &mut Fixture,
    id: ClientId,
    app_id: &str,
    loc: (i32, i32),
    size: (u16, u16),
) -> (Window, ClientSurface) {
    let surface = map_window(f, id, app_id, size);
    let window = window_by_app_id(f, app_id).unwrap();
    f.state()
        .map_window(StageWindow::Client(window.clone()), Point::from(loc), true);
    (window, surface)
}

fn focus(f: &mut Fixture, window: &Window) {
    let serial = SERIAL_COUNTER.next_serial();
    f.state().raise_and_focus(window, serial);
}

fn act(f: &mut Fixture, action: Action) {
    f.state().execute_action(&action);
}

#[test]
fn move_to_snap_docks_at_the_gap_and_nudges_when_nothing_is_further() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((588, 100)),
        "docked snap.gap short of the neighbour's left edge, y untouched"
    );
    assert!(
        f.state().camera_target().is_none(),
        "still fully in view, so the camera stays"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((1412, 100)),
        "the far dock, snap.gap past the neighbour's right edge"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((1432, 100)),
        "no stop further right: nudged by nudge_step"
    );
    assert!(f.state().camera_target().is_none());
}

#[test]
fn move_to_snap_nudges_a_lone_window_by_nudge_step() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (500, 300), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    for (dir, at) in [
        (Direction::Right, (535, 300)),
        (Direction::Down, (535, 335)),
        (Direction::Left, (500, 335)),
        (Direction::Up, (500, 300)),
    ] {
        act(&mut f, Action::MoveToSnap(dir));
        assert_eq!(window_position(&mut f, &a), Point::from(at));
        assert!(
            f.state().camera_target().is_none(),
            "still fully in view, so the camera stays"
        );
    }
}

/// Where `window`'s frame ends on screen, and where a frame riding the usable
/// area's right edge, `outer_gap` canvas px in, ends.
fn right_edges_on_screen(f: &mut Fixture, window: &Window) -> (f64, f64) {
    let rect = f
        .state()
        .visual_frame_rect(&StageWindow::Client(window.clone()))
        .unwrap();
    let (camera, zoom) = (f.state().camera(), f.state().zoom());
    let usable = f.state().get_usable_area();
    let riding = (usable.loc.x + usable.size.w) as f64 - f.state().config.snap_outer_gap * zoom;
    ((rect.x_high - camera.x) * zoom, riding)
}

/// A nudge out of view pans the way an edge pan does: just far enough to show
/// the window whole again, so it rides the usable edge, `outer_gap` in, while
/// the canvas scrolls under it at the zoom it was at, by the step on screen.
#[test]
fn move_to_snap_nudging_out_of_view_pans_just_enough() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // At zoom 0.5 the view spans 3840 canvas px: the right edge at 3806 sits
    // 22 px short of where it rides, at 3828, and a 35 px step covers 70.
    let (a, _) = window_at(&mut f, id, "a", (3406, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    f.state().with_output_state(|os| os.zoom = 0.5);

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((3476, 100)));
    settle(&mut f);
    let (right, riding) = right_edges_on_screen(&mut f, &a);
    assert_eq!(right, riding, "outer_gap in, not centred");
    assert_eq!(f.state().camera(), Point::from((48.0, 0.0)));
    assert_eq!(f.state().zoom(), 0.5);

    for camera_x in [118.0, 188.0] {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        settle(&mut f);
        assert_eq!(
            f.state().camera(),
            Point::from((camera_x, 0.0)),
            "the camera moves by the step"
        );
        let (right, riding) = right_edges_on_screen(&mut f, &a);
        assert_eq!(right, riding, "the window stays put on screen");
    }

    // A held key: each repeat lands before the previous pan has.
    act(&mut f, Action::MoveToSnap(Direction::Right));
    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);
    act(&mut f, Action::MoveToSnap(Direction::Right));
    settle(&mut f);
    assert_eq!(f.state().camera(), Point::from((328.0, 0.0)));
    let (right, riding) = right_edges_on_screen(&mut f, &a);
    assert_eq!(right, riding);
    assert_eq!(f.state().zoom(), 0.5);
}

/// The follow starts at the line it pans to: a step smaller than `outer_gap`
/// scrolls the canvas by the step with the window held on the line, never
/// creeping into the margin and then jumping back out of it.
#[test]
fn move_to_snap_nudging_by_less_than_outer_gap_scrolls_by_the_step() {
    let mut f = Fixture::with_config(config(
        "[navigation]\nnudge_step = 5\n[snap]\nouter_gap = 12.0\n",
    ));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // The frame's right edge sits on the line, 12 px in from 1920.
    let (a, _) = window_at(&mut f, id, "a", (1508, 300), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    for camera_x in [5.0, 10.0, 15.0, 20.0] {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        settle(&mut f);
        assert_eq!(f.state().camera(), Point::from((camera_x, 0.0)));
        let (right, riding) = right_edges_on_screen(&mut f, &a);
        assert_eq!(right, riding, "held on the line");
    }
}

/// The same leftward, where the line lies `outer_gap` in from the usable
/// area's left edge.
#[test]
fn move_to_snap_nudging_left_by_less_than_outer_gap_scrolls_by_the_step() {
    let mut f = Fixture::with_config(config(
        "[navigation]\nnudge_step = 5\n[snap]\nouter_gap = 12.0\n",
    ));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (12, 300), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    for camera_x in [-5.0, -10.0, -15.0] {
        act(&mut f, Action::MoveToSnap(Direction::Left));
        settle(&mut f);
        assert_eq!(f.state().camera(), Point::from((camera_x, 0.0)));
        let rect = f
            .state()
            .visual_frame_rect(&StageWindow::Client(a.clone()))
            .unwrap();
        assert_eq!(rect.x_low - camera_x, 12.0, "held on the line");
    }
}

/// Only the edge leading the move starts a follow: a window in the margin
/// against the right edge, nudged left, leaves the camera alone; nudged
/// right, its first press settles it on the line and the rest scroll by the
/// step.
#[test]
fn move_to_snap_nudging_a_window_in_the_margin_follows_only_its_leading_edge() {
    let mut f = Fixture::with_config(config(
        "[navigation]\nnudge_step = 5\n[snap]\nouter_gap = 12.0\n",
    ));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // Flush against the usable right edge.
    let (a, _) = window_at(&mut f, id, "a", (1520, 300), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(window_position(&mut f, &a), Point::from((1515, 300)));
    assert!(f.state().camera_target().is_none() && f.state().zoom_target().is_none());
    assert_eq!(f.state().camera(), Point::from((0.0, 0.0)));

    for camera_x in [12.0, 17.0, 22.0] {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        settle(&mut f);
        assert_eq!(f.state().camera(), Point::from((camera_x, 0.0)));
        let (right, riding) = right_edges_on_screen(&mut f, &a);
        assert_eq!(right, riding, "held on the line");
    }
}

/// The pan shows the frame the window is drawn with, the one the in-view test
/// judged: a client that kept its size rather than take the one last asked
/// for rides the edge whole, not clipped by the difference.
#[test]
fn move_to_snap_nudging_out_of_view_shows_the_size_the_client_kept() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (1498, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::ShrinkWindow(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (380, 300));
    ack_but_keep_size(&mut f, id, &surface);

    act(&mut f, Action::MoveToSnap(Direction::Right));
    act(&mut f, Action::MoveToSnap(Direction::Right));
    settle(&mut f);
    let (right, riding) = right_edges_on_screen(&mut f, &a);
    assert_eq!(right, riding, "the 400 px it kept, outer_gap in");
}

/// A nudge during a flight pans from where the flight lands: a pan heading
/// down still gets there.
#[test]
fn move_to_snap_nudging_out_of_view_mid_pan_lets_the_pan_land() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (1498, 300), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::PanViewport(Direction::Down));
    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);
    let camera = f.state().camera();
    assert!(
        camera.y > 0.0 && camera.y < 100.0,
        "precondition: the pan is under way: {camera:?}"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    settle(&mut f);
    assert_eq!(f.state().camera(), Point::from((25.0, 100.0)));
}

/// A nudge moves the window along one axis, so the camera follows along that
/// axis only: a window too tall for the view, nudged out past the right edge,
/// scrolls the canvas across and never down to centre it.
#[test]
fn move_to_snap_nudging_a_too_tall_window_out_of_view_pans_only_across() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // Centred vertically, the camera would sit at y -140.
    let (a, _) = window_at(&mut f, id, "a", (1498, -200), (400, 1200));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    settle(&mut f);
    let (right, riding) = right_edges_on_screen(&mut f, &a);
    assert_eq!(right, riding, "outer_gap in from the right edge");
    assert_eq!(f.state().camera(), Point::from((25.0, 0.0)));
}

/// Whether a nudge needs the camera is judged along the move as well: a window
/// too tall to ever show whole, nudged sideways while in view across, leaves
/// the camera alone.
#[test]
fn move_to_snap_nudging_a_too_tall_window_within_view_leaves_the_camera_alone() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (1000, -200), (400, 1200));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((1035, -200)));
    assert!(f.state().camera_target().is_none() && f.state().zoom_target().is_none());
    assert_eq!(
        f.state()
            .with_output_state(|os| os.zoom_animation_anchor.is_none()),
        Some(true),
        "no anchor armed"
    );
    assert_eq!(f.state().camera(), Point::from((0.0, 0.0)));
}

/// The same down the screen: a window too wide for the view, nudged down,
/// leaves the camera alone while its bottom edge stays short of the line,
/// then scrolls the canvas down and never across.
#[test]
fn move_to_snap_nudging_a_too_wide_window_down_pans_only_down() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // Centred across, the camera would sit at x -60.
    let (a, _) = window_at(&mut f, id, "a", (-100, 730), (2000, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Down));
    assert_eq!(window_position(&mut f, &a), Point::from((-100, 765)));
    assert!(f.state().camera_target().is_none() && f.state().zoom_target().is_none());

    act(&mut f, Action::MoveToSnap(Direction::Down));
    settle(&mut f);
    assert_eq!(
        f.state().camera(),
        Point::from((0.0, 32.0)),
        "the bottom edge, at 1100, outer_gap in from 1080"
    );
}

/// A window wider than the view rides the line too, its left part left off
/// screen rather than centred: nudged right while its right edge is short of
/// the line, it leaves the camera alone, then holds that edge on the line
/// press by press.
#[test]
fn move_to_snap_nudging_a_too_wide_window_rides_its_leading_edge_on_the_line() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // The right edge, at 1800, lies 108 px short of the line.
    let (a, _) = window_at(&mut f, id, "a", (-600, 300), (2400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    for _ in 0..3 {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        assert!(f.state().camera_target().is_none() && f.state().zoom_target().is_none());
    }
    assert_eq!(window_position(&mut f, &a), Point::from((-495, 300)));

    for camera_x in [32.0, 67.0, 102.0] {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        settle(&mut f);
        assert_eq!(
            f.state().camera(),
            Point::from((camera_x, 0.0)),
            "no centring jump"
        );
        let (right, riding) = right_edges_on_screen(&mut f, &a);
        assert_eq!(right, riding, "held on the line");
    }
}

/// At zoom 0.5 the view spans 3840 x 2160 canvas px, which a 4000 x 2200
/// window overflows both ways. Its right edge, past the line from the start,
/// rides it press by press, and the camera never moves vertically; the zoom
/// holds.
#[test]
fn move_to_snap_nudging_a_window_too_big_both_ways_rides_its_right_edge_across_only() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (0, 0), (4000, 2200));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    f.state().with_output_state(|os| os.zoom = 0.5);

    // The line lies 3828 canvas px in from the camera, and a 35 px step
    // covers 70.
    for camera_x in [242.0, 312.0, 382.0] {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        settle(&mut f);
        assert_eq!(f.state().camera(), Point::from((camera_x, 0.0)));
        let (right, riding) = right_edges_on_screen(&mut f, &a);
        assert_eq!(right, riding, "held on the line");
        assert_eq!(f.state().zoom(), 0.5);
    }
}

/// A window clipped at the bottom and nudged out past the right edge is
/// brought back across only: the nudge never moved it down, so the camera
/// doesn't scroll down to it either.
#[test]
fn move_to_snap_nudging_out_of_view_leaves_a_clip_at_the_bottom() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (1498, 900), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    settle(&mut f);
    assert_eq!(f.state().camera(), Point::from((25.0, 0.0)));
    let rect = f
        .state()
        .visual_frame_rect(&StageWindow::Client(a.clone()))
        .unwrap();
    assert_eq!(rect.y_high, 1200.0, "still clipped at the bottom");
}

/// A nudge during a flight perpendicular to it leaves that flight's heading
/// alone: a pan down lands where it was going even though the window it
/// reveals across is clipped at the bottom there.
#[test]
fn move_to_snap_nudging_out_of_view_mid_pan_down_lands_the_pan_where_it_was_heading() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // At the pan's landing, y 100, the frame's bottom at 1200 shows at 1100.
    let (a, _) = window_at(&mut f, id, "a", (1498, 900), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::PanViewport(Direction::Down));
    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);
    let camera = f.state().camera();
    assert!(
        camera.y > 0.0 && camera.y < 100.0,
        "precondition: the pan is under way: {camera:?}"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    settle(&mut f);
    assert_eq!(f.state().camera(), Point::from((25.0, 100.0)));
}

/// A nudge during a zoom keeps the zoom's course across the move, not only
/// its landing: down the screen the zoom reset goes on holding the canvas
/// point at the view's centre, though the window lies far below the view.
#[test]
fn move_to_snap_nudging_out_of_view_mid_zoom_keeps_the_zoom_on_course_down() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    // The zoom reset lands the view on x 960..2880; nudged, the frame ends at
    // 2885.
    let (a, _) = window_at(&mut f, id, "a", (2450, 6000), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    f.state().with_output_state(|os| os.zoom = 0.5);
    act(&mut f, Action::ZoomReset);
    let (landing, _) = f.state().landing_view();
    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);
    assert!(
        f.state().zoom() < 1.0,
        "precondition: the zoom is under way"
    );

    let centre_y = f.state().viewport_center_canvas().y;

    act(&mut f, Action::MoveToSnap(Direction::Right));
    for _ in 0..MAX_TICKS {
        if f.state().zoom_target().is_none() {
            break;
        }
        f.state().apply_zoom_animation(TICK);
        f.state().apply_camera_animation(TICK);
        let y = f.state().viewport_center_canvas().y;
        assert!(
            (y - centre_y).abs() < 1e-9,
            "the view's centre stays at canvas y {centre_y}: {y}"
        );
    }
    assert_eq!(f.state().camera(), Point::from((977.0, landing.y)));
    assert_eq!(f.state().zoom(), 1.0);
}

/// Along the move as well, a nudge during a zoom keeps a window far too big
/// for the view on course: its leading edge closes on the line from inside
/// and never passes it, though its centre lies off screen. The zoom reset
/// lands the view on 960..2880 x 540..1620.
#[test]
fn move_to_snap_nudging_mid_zoom_never_carries_a_too_big_window_past_the_line() {
    for (dir, loc) in [
        (Direction::Right, (-3000, -2000)),
        (Direction::Left, (900, -2000)),
        (Direction::Down, (-2000, -4300)),
        (Direction::Up, (-2000, 500)),
    ] {
        let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 35\n"));
        f.add_output(1, (1920, 1080));
        let id = f.add_client();
        let (a, _) = window_at(&mut f, id, "a", loc, (6000, 6000));
        focus(&mut f, &a);
        park_view(&mut f, Point::from((0.0, 0.0)));
        f.state().with_output_state(|os| os.zoom = 0.5);
        act(&mut f, Action::ZoomReset);
        f.state().apply_zoom_animation(TICK);
        f.state().apply_camera_animation(TICK);

        act(&mut f, Action::MoveToSnap(dir.clone()));
        // How far, in screen px, the leading edge sits inside the line as it
        // lies at the landing zoom.
        let inside = |f: &mut Fixture| {
            let rect = f
                .state()
                .visual_frame_rect(&StageWindow::Client(a.clone()))
                .unwrap();
            let (camera, zoom) = (f.state().camera(), f.state().zoom());
            match dir {
                Direction::Right => 1908.0 - (rect.x_high - camera.x) * zoom,
                Direction::Left => (rect.x_low - camera.x) * zoom - 12.0,
                Direction::Down => 1068.0 - (rect.y_high - camera.y) * zoom,
                _ => (rect.y_low - camera.y) * zoom - 12.0,
            }
        };
        for _ in 0..MAX_TICKS {
            if f.state().zoom_target().is_none() {
                break;
            }
            f.state().apply_zoom_animation(TICK);
            f.state().apply_camera_animation(TICK);
            let inside = inside(&mut f);
            assert!(inside > -1e-9, "{dir:?}: {} px past the line", -inside);
        }
        assert!(inside(&mut f).abs() < 1e-9, "{dir:?}: lands on the line");
        assert_eq!(f.state().zoom(), 1.0);
    }
}

/// `nudge_step` takes any non-negative `i32`, so the nudged position has to
/// saturate rather than wrap.
#[test]
fn a_pathological_nudge_step_does_not_overflow_move_to_snap() {
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 2147483647\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, -100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    act(&mut f, Action::MoveToSnap(Direction::Up));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((i32::MAX, i32::MIN))
    );
}

/// `nudge_step = 0` is a no-op through `move-to-snap` too: nothing moves and
/// nothing is queued for the session file.
#[test]
fn a_zero_nudge_step_leaves_move_to_snap_with_no_stop_alone() {
    let tmp = TempDir::new();
    let mut f = Fixture::with_config(config("[navigation]\nnudge_step = 0\n"));
    f.add_output(1, (1920, 1080));
    f.state().session_store.path = Some(tmp.path().join("session.json"));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (500, 300), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    f.state().session_store_write_now();

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((500, 300)));
    assert!(!f.state().session_store_dirty());

    // The step is zero, not the action unreachable.
    f.state().config.nudge_step = 20;
    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((520, 300)));
    f.state().session_store_write_now();
}

/// The alignments are reachable with `[snap] corners` and `centers` off: those
/// knobs tune drag magnetism, not what the keyboard can reach.
#[test]
fn move_to_snap_aligns_with_an_adjacent_neighbour_whatever_the_magnetism_knobs() {
    let mut f = Fixture::with_config(config("[snap]\ncorners = false\ncenters = false\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    // 12 px below `a`, inside the adjacency band, spanning x 300..900.
    window_at(&mut f, id, "b", (300, 412), (600, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    for x in [300, 400, 500] {
        act(&mut f, Action::MoveToSnap(Direction::Right));
        assert_eq!(window_position(&mut f, &a), Point::from((x, 100)));
    }
    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((912, 100)),
        "past the right-edge alignment, on to snap.gap off b's right edge"
    );
}

/// A stop that would land the window on another one is passed over: in a
/// docked row, the docks either side of the middle window are the outer
/// windows' own slots.
#[test]
fn move_to_snap_skips_stops_that_land_on_a_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "n", (512, 100), (400, 300));
    window_at(&mut f, id, "m", (924, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((1336, 100)),
        "past n's and m's slots to the dock beyond m"
    );

    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((100, 100)),
        "and back past both to the dock before n"
    );
}

/// An alignment can land on a window too: `w`, wide below both, offers its
/// left edge, which is where `x` already sits.
#[test]
fn move_to_snap_skips_an_alignment_that_lands_on_a_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    window_at(&mut f, id, "x", (500, 100), (400, 300));
    let (a, _) = window_at(&mut f, id, "a", (912, 100), (400, 300));
    window_at(&mut f, id, "w", (500, 412), (1224, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((88, 100)),
        "past w's left edge, which is x's slot, to the dock before x"
    );
}

/// A window directly below another, already aligned with it every way, steps
/// out sideways to a gap off its side.
#[test]
fn move_to_snap_steps_out_beside_a_window_above() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    window_at(&mut f, id, "a", (600, 100), (400, 300));
    let (b, _) = window_at(&mut f, id, "b", (600, 412), (400, 300));
    focus(&mut f, &b);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(
        window_position(&mut f, &b),
        Point::from((188, 412)),
        "right edge snap.gap off a's left edge, y untouched"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &b),
        Point::from((600, 412)),
        "back under a"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &b),
        Point::from((1012, 412)),
        "left edge snap.gap off a's right edge, y untouched"
    );
}

/// Stepped out beside the window above, `up` walks through its alignments
/// rather than jumping over it.
#[test]
fn move_to_snap_up_from_beside_a_window_above_walks_up_beside_it() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (600, 100), (400, 300));
    let (b, _) = window_at(&mut f, id, "b", (600, 412), (400, 200));
    focus(&mut f, &b);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::MoveToSnap(Direction::Left));

    for (y, aligned) in [(200, "bottom edges"), (150, "centres"), (100, "top edges")] {
        act(&mut f, Action::MoveToSnap(Direction::Up));
        assert_eq!(
            window_position(&mut f, &b),
            Point::from((188, y)),
            "{aligned} aligned with a's"
        );
        let a_frame = f.state().visual_frame_rect(&StageWindow::Client(a.clone()));
        let b_frame = f.state().visual_frame_rect(&StageWindow::Client(b.clone()));
        assert!(!b_frame.unwrap().overlaps(&a_frame.unwrap()), "beside a");
    }
}

/// A side dock is passed over like any stop that would land the window on a
/// third one.
#[test]
fn move_to_snap_skips_a_side_dock_that_lands_on_a_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    window_at(&mut f, id, "a", (1000, 100), (400, 300));
    let (b, _) = window_at(&mut f, id, "b", (1000, 412), (400, 300));
    focus(&mut f, &b);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(
        window_position(&mut f, &b),
        Point::from((588, 412)),
        "precondition: a's side dock"
    );
    act(&mut f, Action::MoveToSnap(Direction::Right));

    // Docked left of `b`, in that slot.
    window_at(&mut f, id, "c", (588, 412), (400, 300));
    focus(&mut f, &b);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(
        window_position(&mut f, &b),
        Point::from((176, 412)),
        "past c's slot to the dock before c"
    );
}

/// A press while the camera is still flying is judged against where the
/// camera is going, not the frame it has reached: the pan already armed
/// would otherwise carry on to the old stop and leave the window behind.
#[test]
fn move_to_snap_mid_pan_brings_the_camera_back_with_the_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (1500, 100), (400, 300));
    window_at(&mut f, id, "c", (800, 100), (400, 300));
    window_at(&mut f, id, "b", (5000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((4588, 100)));

    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);
    assert!(
        f.state().camera_target().is_none() && f.state().zoom_target().is_some(),
        "one frame in, the anchored pan has already dropped its camera target"
    );
    let camera = f.state().camera();
    assert!(
        camera.x < 1212.0 && camera.x + 1920.0 > 1612.0,
        "precondition: the stop back left is in view in this frame: {camera:?}"
    );

    act(&mut f, Action::MoveToSnap(Direction::Left));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((1212, 100)),
        "the dock after c"
    );
    settle(&mut f);
    assert!(
        f.state().window_fully_in_viewport(&a),
        "the camera came back with the window"
    );
}

/// A press during a zoom animation follows the window at the zoom the view is
/// heading to, so the zoom lands instead of freezing where this frame caught it.
#[test]
fn move_to_snap_during_a_zoom_out_lets_the_zoom_land() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (5000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    let resting = f.state().zoom_target();

    act(&mut f, Action::ZoomOut);
    let heading = f.state().zoom_target().expect("zoom-out armed a zoom");
    assert_ne!(Some(heading), resting, "the zoom target moved");
    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);
    assert!(
        f.state().zoom() > heading,
        "precondition: the zoom is under way"
    );

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((4588, 100)));
    settle(&mut f);
    assert_eq!(f.state().zoom(), heading, "the zoom-out landed");
    assert!(
        f.state().window_fully_in_viewport(&a),
        "the camera followed the window"
    );
}

/// A press while the view is flying leaves the flight alone when the stop is
/// on screen where it lands, the way a press at rest leaves a visible stop
/// alone: how quickly the keys follow each other doesn't change the view.
#[test]
fn move_to_snap_during_a_zoom_out_leaves_a_stop_the_view_lands_on_alone() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ZoomOut);
    let landing = f.state().landing_view();
    f.state().apply_zoom_animation(TICK);
    f.state().apply_camera_animation(TICK);

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(window_position(&mut f, &a), Point::from((588, 100)));
    settle(&mut f);
    assert_eq!(
        (f.state().camera(), f.state().zoom()),
        landing,
        "the zoom-out landed where it was heading"
    );
}

/// The camera follows through the directional-focus navigation, so
/// `focus_placement` decides where the window lands on screen.
#[test]
fn move_to_snap_reveals_a_far_stop_on_the_focus_placement_point() {
    let mut f = Fixture::with_config(config(
        "focus_placement = \"left\"\n[snap]\nouter_gap = 0.0\n",
    ));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, _) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (5000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    let resting = f.state().camera();

    act(&mut f, Action::MoveToSnap(Direction::Right));

    assert_eq!(
        window_position(&mut f, &a),
        Point::from((4588, 100)),
        "a neighbour screens away is a valid target"
    );
    let target = f
        .state()
        .camera_target()
        .expect("the window left the viewport, so the camera follows");
    assert_ne!(target, resting);

    settle(&mut f);
    let rect = f
        .state()
        .visual_frame_rect(&StageWindow::Client(a.clone()))
        .unwrap();
    let camera = f.state().camera();
    let usable = f.state().get_usable_area();
    let x_low = (rect.x_low - camera.x) * f.state().zoom();
    assert!(
        (x_low - usable.loc.x as f64).abs() < 1e-6,
        "focus_placement = left parks the frame's left edge on the usable area's left: {x_low}"
    );
}

#[test]
fn resize_to_snap_walks_the_right_edge_out_to_a_dock_and_back() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    // Below `a`, offering its right edge at 700 as an alignment.
    window_at(&mut f, id, "c", (100, 412), (600, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    let before = window_position(&mut f, &a);

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (600, 300),
        "the right edge aligns with c's"
    );
    assert_eq!(window_position(&mut f, &a), before, "the left edge holds");

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (888, 300),
        "then stops snap.gap short of b"
    );

    let configures = configure_count(&mut f, id, &surface);
    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(
        configure_count(&mut f, id, &surface),
        configures,
        "docked: growing on would cover b, so no configure at all"
    );

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (600, 300),
        "back to the previous stop"
    );

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (580, 300),
        "no stop inside the window: in by resize_step"
    );
    assert_eq!(window_position(&mut f, &a), before);
    assert!(
        f.state().camera_target().is_none(),
        "a resize never moves the camera"
    );
}

/// With no stop inside, a shrink steps the right or bottom edge in by
/// `resize_step`, the left and top edges holding, down to the client's
/// minimum.
#[test]
fn resize_to_snap_shrinks_by_resize_step_with_no_stop_inside() {
    let mut f = Fixture::with_config(config("[navigation]\nresize_step = 30\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let surface = map_window_with_limits(&mut f, id, "a", (400, 300), (370, 270), (0, 0));
    let a = window_by_app_id(&mut f, "a").unwrap();
    f.state().map_window(
        StageWindow::Client(a.clone()),
        Point::from((100, 100)),
        true,
    );
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(last_configured(&mut f, id, &surface), (370, 300));
    ack_with_size(&mut f, id, &surface, (370, 300));
    act(&mut f, Action::ResizeToSnap(Direction::Up));
    assert_eq!(last_configured(&mut f, id, &surface), (370, 270));
    ack_with_size(&mut f, id, &surface, (370, 270));
    assert_eq!(window_position(&mut f, &a), Point::from((100, 100)));

    let configures = configure_count(&mut f, id, &surface);
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    act(&mut f, Action::ResizeToSnap(Direction::Up));
    assert_eq!(
        configure_count(&mut f, id, &surface),
        configures,
        "at the minimum: no configure"
    );
    assert_eq!(window_position(&mut f, &a), Point::from((100, 100)));
}

/// The step comes off the edge the walk starts from, not the one the client
/// kept: a client that declined the last request would otherwise be asked to
/// grow past it.
#[test]
fn resize_to_snap_shrinks_by_resize_step_from_an_outstanding_request() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    // Below `a`, offering its right edge at 300 as an alignment.
    window_at(&mut f, id, "c", (100, 412), (200, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(last_configured(&mut f, id, &surface), (200, 300));
    ack_but_keep_size(&mut f, id, &surface);

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (180, 300),
        "resize_step in from the 200 asked for, not from the 400 kept"
    );
}

/// `resize_step = 0` is a no-op through `resize-to-snap` too, even with a
/// request outstanding that a zero step would ask for again.
#[test]
fn a_zero_resize_step_leaves_resize_to_snap_with_no_stop_alone() {
    let mut f = Fixture::with_config(config("[navigation]\nresize_step = 0\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    // Below `a`, offering its right edge at 300 as an alignment.
    window_at(&mut f, id, "c", (100, 412), (200, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (200, 300),
        "precondition: the stop"
    );
    ack_but_keep_size(&mut f, id, &surface);

    let configures = configure_count(&mut f, id, &surface);
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(configure_count(&mut f, id, &surface), configures);
    assert_eq!(window_position(&mut f, &a), Point::from((100, 100)));

    // The step is zero, not the action unreachable.
    f.state().config.resize_step = 20;
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(last_configured(&mut f, id, &surface), (180, 300));
}

/// The fill edge: the usable area's edge, `snap.outer_gap` in.
#[test]
fn resize_to_snap_with_no_neighbour_grows_to_the_fill_edge() {
    let mut f = Fixture::with_config(config("[snap]\nouter_gap = 12.0\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (1808, 300));
    act(&mut f, Action::ResizeToSnap(Direction::Down));
    assert_eq!(last_configured(&mut f, id, &surface), (1808, 968));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((100, 100)),
        "both edges grew away from the top-left"
    );
}

/// Only a window the frame would newly cover is in the way: one it already
/// overlaps doesn't stop it short of the fill edge.
#[test]
fn resize_to_snap_grows_over_a_window_it_already_overlaps() {
    let mut f = Fixture::with_config(config("[snap]\nouter_gap = 12.0\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (400, 200), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (1808, 300));
}

/// A window diagonally ahead isn't in the way: the edge walks on to its far
/// edge rather than docking short of it.
#[test]
fn resize_to_snap_grows_past_a_window_diagonally_ahead() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    // Below and right of `a`, offering its right edge at 1100 as an alignment.
    window_at(&mut f, id, "c", (700, 412), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (1000, 300));
}

/// A cell-rounding client answers short of the stop or past it; the walk goes
/// on from the stop it asked for, else every press re-asks for the same size.
#[test]
fn resize_to_snap_walks_on_past_a_cell_rounded_answer() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    // Above and below `a`, offering their right edges at 350 and 700.
    window_at(&mut f, id, "d", (100, -212), (250, 300));
    window_at(&mut f, id, "c", (100, 412), (600, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (600, 300));
    ack_with_size(&mut f, id, &surface, (595, 300));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (888, 300),
        "rounded down: on to the dock, not back to c's edge"
    );
    ack_with_size(&mut f, id, &surface, (883, 300));

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(last_configured(&mut f, id, &surface), (600, 300));
    ack_with_size(&mut f, id, &surface, (605, 300));

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (250, 300),
        "rounded up: on to d's edge, not back to c's"
    );
    assert_eq!(window_position(&mut f, &a), Point::from((100, 100)));
}

/// What a grow would cover is judged from the frame the client committed, not
/// the one it was asked for: a window docked against the rounded edge sits
/// between the two, and growing on would cover it.
#[test]
fn resize_to_snap_stops_at_a_window_docked_against_a_rounded_answer() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    // Below `a`, offering its right edge at 700 as an alignment.
    window_at(&mut f, id, "c", (100, 412), (600, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (600, 300));
    ack_with_size(&mut f, id, &surface, (585, 300));
    // Docked snap.gap off the edge `a` committed, at 685.
    window_at(&mut f, id, "n", (697, 100), (400, 300));
    focus(&mut f, &a);
    // The refocus configure carries the size asked for again, and the client
    // rounds it the same way.
    ack_with_size(&mut f, id, &surface, (585, 300));
    park_view(&mut f, Point::from((0.0, 0.0)));

    let configures = configure_count(&mut f, id, &surface);
    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(
        configure_count(&mut f, id, &surface),
        configures,
        "docked against n already: no configure"
    );
}

/// A rounded answer that lands exactly on another stop has reached it: the
/// walk goes on past it rather than re-asking for the size the window already
/// has.
#[test]
fn resize_to_snap_counts_a_stop_the_rounding_landed_on_as_reached() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    // Above and below `a`, offering their right edges at 350 and 983.
    window_at(&mut f, id, "d", (100, -212), (250, 300));
    window_at(&mut f, id, "c", (100, 412), (883, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (883, 300));
    ack_with_size(&mut f, id, &surface, (880, 300));
    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (888, 300));
    ack_with_size(&mut f, id, &surface, (883, 300));

    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (250, 300),
        "sitting on c's edge already, the walk goes on to d's"
    );
}

/// The fence is the usable area in canvas px, so at zoom 0.5 it reaches twice
/// as far — and `outer_gap` stays canvas px, as `fill-window` measures it.
#[test]
fn resize_to_snap_grows_to_the_fill_edge_at_the_current_zoom() {
    let mut f = Fixture::with_config(config("[snap]\nouter_gap = 12.0\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    f.state().with_output_state(|os| os.zoom = 0.5);

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (3728, 300));
    act(&mut f, Action::ResizeToSnap(Direction::Down));
    assert_eq!(last_configured(&mut f, id, &surface), (3728, 2048));
}

/// A title bar sits above the content: the bottom edge the walk moves is the
/// content's own, and a neighbour's frame starts a bar above its content.
#[test]
fn resize_to_snap_down_measures_title_bars() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    let (b, _) = window_at(&mut f, id, "b", (100, 700), (400, 300));
    // Beside `a`, offering its bottom edge at 300 as an alignment.
    window_at(&mut f, id, "c", (512, 100), (400, 200));
    give_ssd(&mut f, &a);
    give_ssd(&mut f, &b);
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::ResizeToSnap(Direction::Down));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (400, 563),
        "b's frame starts at 675, so the bottom docks at 663"
    );
    assert_eq!(window_position(&mut f, &a), Point::from((100, 100)));

    let configures = configure_count(&mut f, id, &surface);
    act(&mut f, Action::ResizeToSnap(Direction::Down));
    assert_eq!(configure_count(&mut f, id, &surface), configures);

    act(&mut f, Action::ResizeToSnap(Direction::Up));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (400, 200),
        "up to c's bottom edge, the bar staying above the content"
    );
}

#[test]
fn a_stand_in_resizes_and_moves_to_snap_like_a_client() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    let sid = f.state().insert_suspended_for_test(
        1,
        Point::from((100, 100)),
        Size::from((400, 300)),
        "s",
        "S",
    );
    f.state().focus_and_raise_suspended(sid);
    park_view(&mut f, Point::from((0.0, 0.0)));
    let s = f.state().find_suspended(sid).unwrap();
    let elem = StageWindow::Suspended(s.clone());

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(s.size.get(), Size::from((888, 300)));
    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(s.size.get(), Size::from((888, 300)), "docked: a no-op");

    // Docked at snap.gap already, the next stop right is the neighbour's far
    // side.
    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        f.state().stage.position_of(&elem),
        Some(Point::from((1412, 100))),
        "the stand-in's bar sits above its body, so the horizontal stop is a client's"
    );

    f.state().dismiss_suspended(sid);
}

/// The action prelude exits fullscreen and only *sends* the restore, so
/// committed geometry is still the output's size when the move measures.
#[test]
fn move_to_snap_during_fullscreen_measures_the_restored_window() {
    let mut f = Fixture::new();
    let output = f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));
    let key = super::server_surface(&a).id();

    f.state().enter_fullscreen(&a, Some(output));
    f.double_roundtrip(id);
    super::adopt_last_configure(&mut f, id, &surface);
    assert!(f.state().is_fullscreen(), "precondition: fullscreen");

    act(&mut f, Action::MoveToSnap(Direction::Right));

    assert!(!f.state().is_fullscreen());
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((588, 100)),
        "docked by its restored 400 px width, not the 1920 px it still commits"
    );
    assert!(
        f.state().camera_target().is_none() && f.state().zoom_target().is_none(),
        "and judged in view by that width too, so the camera stays"
    );
    assert!(
        !f.state().pending_recenter.contains_key(&key),
        "the exit's owed recenter went with the move"
    );
}

#[test]
fn resize_to_snap_leaves_a_window_outside_the_viewport_alone() {
    let mut f = Fixture::with_config(config("[snap]\nouter_gap = 0.0\n"));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((3000.0, 0.0)));
    let configures = configure_count(&mut f, id, &surface);

    act(&mut f, Action::ResizeToSnap(Direction::Right));
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        configure_count(&mut f, id, &surface),
        configures,
        "off screen: no configure, a shrink included"
    );

    park_view(&mut f, Point::from((0.0, 0.0)));
    act(&mut f, Action::ResizeToSnap(Direction::Right));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (1820, 300),
        "back in view, the same press grows to the usable edge"
    );
}

#[test]
fn a_stand_in_moves_down_by_its_frame_and_lands_its_body_below_the_bar() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    window_at(&mut f, id, "b", (100, 1000), (400, 300));
    let sid = f.state().insert_suspended_for_test(
        1,
        Point::from((100, 100)),
        Size::from((400, 300)),
        "s",
        "S",
    );
    f.state().focus_and_raise_suspended(sid);
    park_view(&mut f, Point::from((0.0, 0.0)));
    let s = f.state().find_suspended(sid).unwrap();
    let elem = StageWindow::Suspended(s);

    act(&mut f, Action::MoveToSnap(Direction::Down));

    // Frame 75..400 (a 25 px bar) docks its bottom at 988: origin 663, body
    // 25 lower.
    assert_eq!(
        f.state().stage.position_of(&elem),
        Some(Point::from((100, 688)))
    );

    f.state().dismiss_suspended(sid);
}

#[test]
fn a_border_widens_the_frames_the_stops_measure() {
    let mut f = Fixture::with_config(config(
        "[snap]\nouter_gap = 0.0\n[decorations]\nborder_width = 2\n",
    ));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, "a", (100, 100), (400, 300));
    window_at(&mut f, id, "b", (1000, 100), (400, 300));
    focus(&mut f, &a);
    park_view(&mut f, Point::from((0.0, 0.0)));

    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(
        window_position(&mut f, &a),
        Point::from((584, 100)),
        "frames 98..502 and 998..1402 dock at 986, so the content starts at 584"
    );

    act(&mut f, Action::ResizeToSnap(Direction::Down));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (400, 980),
        "the frame grows to 1082, a border past the usable edge at outer_gap 0"
    );
}
