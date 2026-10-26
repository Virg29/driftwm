//! `nudge_step` and `resize_step` are screen px: a step covers `step / zoom`
//! canvas px at the zoom the view showing the element lands at, never rounds
//! down to nothing, and a step of 0 stays a no-op.

use smithay::desktop::Window;
use smithay::output::Output;
use smithay::utils::{Point, SERIAL_COUNTER};
use wayland_client::protocol::wl_surface::WlSurface as ClientSurface;

use super::client::ClientId;
use super::{
    Fixture, TICK, ack_with_size, config, configure_count, last_configured, map_window,
    window_by_app_id, window_position,
};
use crate::state::{StageWindow, output_state};
use driftwm::config::{Action, Direction};

/// Park the viewport at the origin at `zoom` with nothing in flight, so the
/// landing zoom is `zoom` rather than the `Some(1.0)` a map leaves behind. A
/// zoom above 1 is reachable only this way: input clamps to `MAX_ZOOM`.
fn park_view(f: &mut Fixture, zoom: f64) {
    f.state().with_output_state(|os| {
        os.camera = Point::from((0.0, 0.0));
        os.camera_target = None;
        os.zoom = zoom;
        os.zoom_target = None;
        os.zoom_animation_anchor = None;
    });
}

fn park_output(output: &Output, camera: (f64, f64), zoom: f64) {
    let mut os = output_state(output);
    os.camera = Point::from(camera);
    os.camera_target = None;
    os.zoom = zoom;
    os.zoom_target = None;
    os.zoom_animation_anchor = None;
}

fn window_at(
    f: &mut Fixture,
    id: ClientId,
    loc: (i32, i32),
    size: (u16, u16),
) -> (Window, ClientSurface) {
    let surface = map_window(f, id, "a", size);
    let window = window_by_app_id(f, "a").unwrap();
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

/// One focused 400x300 window at (100, 100) on a 1920x1080 output parked at
/// `zoom`.
fn scene(toml: &str, zoom: f64) -> (Fixture, ClientId, Window, ClientSurface) {
    scene_sized(toml, zoom, (400, 300))
}

fn scene_sized(
    toml: &str,
    zoom: f64,
    size: (u16, u16),
) -> (Fixture, ClientId, Window, ClientSurface) {
    let mut f = Fixture::with_config(config(toml));
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let (window, surface) = window_at(&mut f, id, (100, 100), size);
    focus(&mut f, &window);
    park_view(&mut f, zoom);
    (f, id, window, surface)
}

const NUDGE_20: &str = "[navigation]\nnudge_step = 20\n";
const RESIZE_20: &str = "[navigation]\nresize_step = 20\n";

fn moved_by(f: &mut Fixture, window: &Window, from: (i32, i32)) -> (i32, i32) {
    let now = window_position(f, window);
    (now.x - from.0, now.y - from.1)
}

/// Zoomed out a step covers more canvas, zoomed in less, the same on screen.
#[test]
fn nudge_window_moves_step_over_zoom_canvas_px() {
    for (zoom, step) in [(0.5, 40), (1.0, 20), (2.0, 10)] {
        let (mut f, _, a, _) = scene(NUDGE_20, zoom);
        act(&mut f, Action::NudgeWindow(Direction::Right));
        assert_eq!(moved_by(&mut f, &a, (100, 100)), (step, 0), "zoom {zoom}");
        act(&mut f, Action::NudgeWindow(Direction::Up));
        assert_eq!(
            moved_by(&mut f, &a, (100, 100)),
            (step, -step),
            "zoom {zoom}"
        );
    }
}

/// The diagonal converts the step to canvas px first, then projects it: 5 px
/// at zoom 0.5 is 10 canvas px, 7 per axis, where projecting first would
/// round to 4 screen px, 8 canvas.
#[test]
fn nudge_window_diagonal_projects_the_canvas_step() {
    let (mut f, _, a, _) = scene("[navigation]\nnudge_step = 5\n", 0.5);
    act(&mut f, Action::NudgeWindow(Direction::DownRight));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (7, 7));
}

/// 7 px at zoom 2 rounds to 4 canvas px, 3 per axis, where dividing the
/// projected step by the zoom would give 2.
#[test]
fn nudge_window_diagonal_at_zoom_two_projects_the_canvas_step() {
    let (mut f, _, a, _) = scene("[navigation]\nnudge_step = 7\n", 2.0);
    act(&mut f, Action::NudgeWindow(Direction::UpLeft));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (-3, -3));
}

/// The nudge fallback steps in screen px like `nudge-window`.
#[test]
fn move_to_snap_without_a_stop_moves_step_over_zoom_canvas_px() {
    for (zoom, step) in [(0.5, 40), (1.0, 20), (2.0, 10)] {
        let (mut f, _, a, _) = scene(NUDGE_20, zoom);
        act(&mut f, Action::MoveToSnap(Direction::Right));
        assert_eq!(moved_by(&mut f, &a, (100, 100)), (step, 0), "zoom {zoom}");
        act(&mut f, Action::MoveToSnap(Direction::Down));
        assert_eq!(
            moved_by(&mut f, &a, (100, 100)),
            (step, step),
            "zoom {zoom}"
        );
    }
}

/// Both directions of a resize step convert alike: a shrink keeps its sign
/// through the conversion rather than clamping up to a 1 px grow.
#[test]
fn grow_and_shrink_window_change_size_by_step_over_zoom() {
    for (zoom, step) in [(0.5, 40), (1.0, 20), (2.0, 10)] {
        let (mut f, id, _, surface) = scene(RESIZE_20, zoom);
        act(&mut f, Action::GrowWindow(Direction::Right));
        assert_eq!(
            last_configured(&mut f, id, &surface),
            (400 + step, 300),
            "grow at zoom {zoom}"
        );
        ack_with_size(&mut f, id, &surface, (400 + step as u16, 300));
        act(&mut f, Action::ShrinkWindow(Direction::Down));
        assert_eq!(
            last_configured(&mut f, id, &surface),
            (400 + step, 300 - step),
            "shrink at zoom {zoom}"
        );
    }
}

/// The shrink fallback steps in screen px like `shrink-window`.
#[test]
fn resize_to_snap_shrinks_by_step_over_zoom_with_no_stop_inside() {
    for (zoom, step) in [(0.5, 40), (2.0, 10)] {
        let (mut f, id, _, surface) = scene(RESIZE_20, zoom);
        act(&mut f, Action::ResizeToSnap(Direction::Left));
        assert_eq!(
            last_configured(&mut f, id, &surface),
            (400 - step, 300),
            "left at zoom {zoom}"
        );
        ack_with_size(&mut f, id, &surface, (400 - step as u16, 300));
        act(&mut f, Action::ResizeToSnap(Direction::Up));
        assert_eq!(
            last_configured(&mut f, id, &surface),
            (400 - step, 300 - step),
            "up at zoom {zoom}"
        );
    }
}

/// A zoom out armed but barely ticked: the live zoom is still between 1 and
/// the target, and a step must already be sized for the target.
fn zoom_out_in_flight(toml: &str) -> (Fixture, ClientId, Window, ClientSurface, f64) {
    let (mut f, id, a, surface) = scene(toml, 1.0);
    act(&mut f, Action::ZoomOut);
    f.state().apply_zoom_animation(TICK);
    let target = f.state().zoom_target().expect("a zoom flight is armed");
    let live = f.state().zoom();
    assert!(
        target < live && live < 1.0,
        "precondition: live zoom {live} is mid-flight toward {target}"
    );
    (f, id, a, surface, target)
}

#[test]
fn nudge_window_mid_zoom_flight_uses_the_target_zoom() {
    let (mut f, _, a, _, target) = zoom_out_in_flight("[zoom]\nstep = 2.0\n");
    assert_eq!(target, 0.5, "precondition: the flight lands at 0.5");
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (40, 0));
}

#[test]
fn grow_window_mid_zoom_flight_uses_the_target_zoom() {
    let (mut f, id, _, surface, target) =
        zoom_out_in_flight("[zoom]\nstep = 2.0\n[navigation]\nresize_step = 20\n");
    assert_eq!(target, 0.5, "precondition: the flight lands at 0.5");
    act(&mut f, Action::GrowWindow(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (440, 300));
}

/// A quarter px rounds to nothing, but a configured step always moves.
#[test]
fn a_one_px_nudge_step_still_moves_one_canvas_px_at_high_zoom() {
    let (mut f, _, a, _) = scene_sized("[navigation]\nnudge_step = 1\n", 4.0, (200, 100));
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (1, 0), "nudge-window");
    act(&mut f, Action::MoveToSnap(Direction::Down));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (1, 1), "move-to-snap");
}

/// As for the nudge, for both resize steps.
#[test]
fn a_one_px_resize_step_still_resizes_one_canvas_px_at_high_zoom() {
    let (mut f, id, _, surface) = scene_sized("[navigation]\nresize_step = 1\n", 4.0, (200, 100));
    act(&mut f, Action::GrowWindow(Direction::Right));
    assert_eq!(last_configured(&mut f, id, &surface), (201, 100), "grow");
    ack_with_size(&mut f, id, &surface, (201, 100));
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (200, 100),
        "resize-to-snap"
    );
}

/// The at-least-1 clamp never turns a configured 0 into a step.
#[test]
fn a_zero_nudge_step_moves_nothing_zoomed_out() {
    let (mut f, _, a, _) = scene("[navigation]\nnudge_step = 0\n", 0.5);
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (0, 0), "nudge-window");
    act(&mut f, Action::MoveToSnap(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (0, 0), "move-to-snap");
}

#[test]
fn a_zero_resize_step_configures_nothing_zoomed_out() {
    let (mut f, id, _, surface) = scene("[navigation]\nresize_step = 0\n", 0.5);
    let configures = configure_count(&mut f, id, &surface);
    act(&mut f, Action::GrowWindow(Direction::Right));
    act(&mut f, Action::ShrinkWindow(Direction::Down));
    act(&mut f, Action::ResizeToSnap(Direction::Left));
    assert_eq!(configure_count(&mut f, id, &surface), configures);
}

fn two_outputs(toml: &str) -> (Fixture, Output, Output, Window) {
    let mut f = Fixture::with_config(config(toml));
    f.skip_baseline_check();
    let first = f.add_output(1, (1920, 1080));
    let second = f.add_output(2, (1920, 1080));
    let id = f.add_client();
    let (window, _) = window_at(&mut f, id, (100, 100), (400, 300));
    focus(&mut f, &window);
    f.state().focused_output = Some(second.clone());
    (f, first, second, window)
}

/// Both views show the window; the active output's zoom wins over the one
/// registered first.
#[test]
fn nudge_prefers_the_active_output_when_it_shows_the_window() {
    let (mut f, first, second, a) = two_outputs(NUDGE_20);
    park_output(&first, (0.0, 0.0), 1.0);
    park_output(&second, (0.0, 0.0), 0.5);
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (40, 0));
}

/// The active output looks elsewhere, so the output that does show the window
/// sizes the step.
#[test]
fn nudge_uses_the_output_showing_the_window_when_the_active_one_does_not() {
    let (mut f, first, second, a) = two_outputs(NUDGE_20);
    park_output(&first, (0.0, 0.0), 0.5);
    park_output(&second, (50_000.0, 50_000.0), 1.0);
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (40, 0));
}

/// The active output counts as showing the window where its pan lands, not
/// where this frame caught it: a held key outruns the camera riding it.
#[test]
fn nudge_judges_the_active_output_where_its_pan_lands() {
    let (mut f, first, second, a) = two_outputs(NUDGE_20);
    park_output(&first, (0.0, 0.0), 1.0);
    park_output(&second, (50_000.0, 50_000.0), 0.5);
    output_state(&second).camera_target = Some(Point::from((0.0, 0.0)));
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (100, 100)), (40, 0));
}

/// The window is judged at the size its placement assumes, as move-to-snap
/// frames it: asked to shrink from 2000 to 400 wide and not yet acked, its
/// centre is at x 1200, which the active output shows, not at the committed
/// 2000, which only the other one does.
#[test]
fn the_active_output_is_judged_at_the_size_the_placement_assumes() {
    let mut f = Fixture::with_config(config(
        "[navigation]\nnudge_step = 20\nresize_step = 1600\n",
    ));
    f.skip_baseline_check();
    let first = f.add_output(1, (1920, 1080));
    let second = f.add_output(2, (1920, 1080));
    let id = f.add_client();
    let (a, surface) = window_at(&mut f, id, (1000, 100), (2000, 300));
    focus(&mut f, &a);
    f.state().focused_output = Some(second.clone());
    park_output(&first, (0.0, 0.0), 1.0);
    park_output(&second, (0.0, 0.0), 1.0);
    act(&mut f, Action::ShrinkWindow(Direction::Right));
    assert_eq!(
        last_configured(&mut f, id, &surface),
        (400, 300),
        "precondition: a shrink is outstanding"
    );

    park_output(&first, (0.0, 0.0), 0.5);
    act(&mut f, Action::NudgeWindow(Direction::Right));
    assert_eq!(moved_by(&mut f, &a, (1000, 100)), (20, 0));
}
