//! `move-to-snap` / `resize-to-snap`: the keyboard walks between the stops
//! drag snapping offers, every family at any distance, and a move also to the
//! side docks a gap off an adjacent neighbour's sides.

use std::collections::HashSet;

use smithay::utils::{Logical, Point};

use super::{DriftWm, MinimalPan, NavZoom, StageWindow};
use driftwm::config::Direction;
use driftwm::layout::snap::{self, SnapRect, StopQuery};
use driftwm::stage::StageElement;

impl DriftWm {
    /// Move the focused canvas element along `dir` to the nearest stop past
    /// where it sits, and bring the camera along if that leaves the viewport.
    /// With no stop that way it nudges by `nudge_step` instead, the camera
    /// scrolling along with it once its leading edge reaches `outer_gap` from
    /// the usable edge.
    pub(crate) fn move_to_snap_focused(&mut self, dir: &Direction) {
        let Some((horizontal, forward)) = axis_of(dir) else {
            return;
        };
        let Some(element) = self.focused_element().filter(|e| self.is_canvas_window(e)) else {
            return;
        };
        let Some(loc) = self.stage.position_of(&element) else {
            return;
        };

        #[allow(clippy::mutable_key_type)]
        let excludes = HashSet::new();
        let (others, bar, bw) = self.snap_targets(&element, &excludes);
        // The size the placement assumes, not committed geometry: the action
        // prelude may have just exited fullscreen, and the client goes on
        // reporting the output's size until it acks the restore.
        let size = self.placed_element_size(&element);
        let frame = super::fit::snap_rect_at(loc, size, bar, bw);
        let q = StopQuery {
            side_docks: true,
            ..self.stop_query(&frame, horizontal)
        };
        let (from, extent) = if horizontal {
            (frame.x_low, frame.x_high - frame.x_low)
        } else {
            (frame.y_low, frame.y_high - frame.y_low)
        };
        let stop = snap::next_origin(from, forward, extent, &others, &q);
        let new_loc = match stop {
            // The stop places the visual frame; the stage holds the content
            // top-left, one border in and the bar down from it.
            Some(origin) if horizontal => Point::from((origin as i32 + bw, loc.y)),
            Some(origin) => Point::from((loc.x, origin as i32 + bar + bw)),
            // No clear stop means no window ahead in the perpendicular span:
            // the one reaching furthest that way would offer a clear far dock.
            // So the nudge can't land on a window either.
            None => {
                let step = self.canvas_step(&element, self.config.nudge_step);
                if step == 0 {
                    return;
                }
                let step = if forward { step } else { -step };
                if horizontal {
                    Point::from((loc.x.saturating_add(step), loc.y))
                } else {
                    Point::from((loc.x, loc.y.saturating_add(step)))
                }
            }
        };
        self.keyboard_move_element(&element, loc, new_loc);

        // Judged where a flight still under way lands, not at its current
        // frame: a pan heading for the previous stop would leave this one.
        let (camera, zoom) = self.landing_view();
        let landed = super::fit::snap_rect_at(new_loc, size, bar, bw);
        if stop.is_some() {
            if !self.frame_fully_in_view_at(&landed, camera, zoom) {
                self.navigate_to_element(&element, NavZoom::Keep);
            }
            return;
        }
        // As an edge pan carries a dragged window: it rides the `outer_gap`
        // line while the canvas scrolls under it, only the way the window
        // went, where parking every step would throw the view around. Shown
        // at the size just judged, which a client that declined a resize is
        // still drawn at.
        let pan = MinimalPan::LeadingEdge {
            horizontal,
            forward,
        };
        if self.minimal_pan_moves(&element, &landed, camera, zoom, pan) {
            self.pan_minimally_to_element(&element, &landed, camera, zoom, pan);
        }
    }

    /// Move `element` from `from` to `to`, re-anchoring it: its fill restore
    /// point and any owed recenter go stale, and its position is session state.
    pub(crate) fn keyboard_move_element(
        &mut self,
        element: &StageWindow,
        from: Point<i32, Logical>,
        to: Point<i32, Logical>,
    ) {
        self.stage.clear_fill(element);
        self.drop_owed_recenter(element);
        self.map_window(element.clone(), to, false);
        self.animate_element_move_from(element, from, None);
        self.session_store_mark_dirty();
    }

    /// The canvas px a configured `step` of screen px covers for `element`,
    /// at the zoom the view showing it lands at, as a drag feels at that zoom.
    /// The active output's view when it lands showing the element, since that
    /// is the view move-to-snap scrolls; else `output_for_window`'s. Whole px,
    /// and never rounded down to nothing.
    pub(crate) fn canvas_step(&self, element: &StageWindow, step: i32) -> i32 {
        if step == 0 {
            return 0;
        }
        let center = self.stage.position_of(element).map(|loc| {
            let size = element.size();
            Point::from((
                loc.x.saturating_add(size.w / 2),
                loc.y.saturating_add(size.h / 2),
            ))
        });
        let active = self.active_output().filter(|output| {
            let (camera, zoom) = self.landing_view_on(output);
            let view = driftwm::canvas::visible_canvas_rect(
                camera.to_i32_round(),
                super::output_logical_size(output),
                zoom,
            );
            center.is_some_and(|c| view.contains(c))
        });
        let zoom = active
            .or_else(|| self.output_for_window(element))
            .map_or(1.0, |output| self.landing_view_on(&output).1);
        ((f64::from(step) / zoom).round() as i32).max(1)
    }

    /// Resize the focused canvas element by walking its right or bottom edge:
    /// `right` / `down` grow it out to the next stop, never past the usable
    /// area or over another window; `left` / `up` pull it back to the previous
    /// one, or by `resize_step` with none inside the window.
    pub(crate) fn resize_to_snap_focused(&mut self, dir: &Direction) {
        let Some((horizontal, grow)) = axis_of(dir) else {
            return;
        };
        let Some((element, start)) = self.step_target() else {
            return;
        };
        #[allow(clippy::mutable_key_type)]
        let excludes = HashSet::new();
        let (others, bar, bw) = self.snap_targets(&element, &excludes);
        let frame = super::fit::snap_rect_at(start.loc, start.size, bar, bw);
        let Some(output) = self.output_for_window(&element) else {
            return;
        };
        // The fence `fill-window` grows inside, refused the same way for a
        // window lying entirely outside it: `output_for_window` falls back to
        // the active output for a window no viewport shows, and that
        // viewport's far edge is no bound for a window screens away.
        let fence = self
            .usable_canvas_bounds(&output)
            .inset(self.config.outer_frame_inset(bw));
        if !frame.overlaps(&fence) {
            return;
        }
        let q = self.stop_query(&frame, horizontal);
        // A cell-rounding client answers short of, or past, the stop it was
        // asked for. Walking on from that answer would re-ask for the same
        // stop on every press, so the walk starts from whichever of the
        // answered and requested edges is further along.
        let requested =
            super::fit::snap_rect_at(start.loc, self.requested_element_size(&element), bar, bw);
        let (edge, requested_edge, anchor, bound) = if horizontal {
            (frame.x_high, requested.x_high, frame.x_low, fence.x_high)
        } else {
            (frame.y_high, requested.y_high, frame.y_low, fence.y_high)
        };
        let target = if grow {
            snap::next_grow_line(edge, edge.max(requested_edge), bound, &others, &q)
        } else {
            let from = edge.min(requested_edge);
            // A zero step isn't a reliable no-op: from a declined request it
            // would ask for that request again.
            let step = self.canvas_step(&element, self.config.resize_step);
            snap::next_shrink_line(from, anchor, bound, &others, &q)
                .or_else(|| (step > 0).then_some(from - step as f64))
        };
        let Some(target) = target else {
            return;
        };
        let axis = if horizontal {
            Direction::Right
        } else {
            Direction::Down
        };
        self.step_resize(&element, start, &axis, (target - edge).round() as i32);
    }

    /// The query for `frame`'s stops along one axis, with the alignment band a
    /// drag at zoom 1 would use.
    fn stop_query(&self, frame: &SnapRect, horizontal: bool) -> StopQuery {
        let (perp_low, perp_high) = if horizontal {
            (frame.y_low, frame.y_high)
        } else {
            (frame.x_low, frame.x_high)
        };
        StopQuery {
            horizontal,
            perp_low,
            perp_high,
            gap: self.config.snap_gap,
            align_band: self.config.snap_gap + self.config.snap_distance,
            // The magnetism knobs tune a drag, not what the keyboard can reach.
            same_edge: true,
            edge_center: true,
            side_docks: false,
        }
    }
}

/// `(horizontal, toward higher coordinates)` for a cardinal direction. The
/// diagonals never parse for these actions.
fn axis_of(dir: &Direction) -> Option<(bool, bool)> {
    match dir {
        Direction::Left => Some((true, false)),
        Direction::Right => Some((true, true)),
        Direction::Up => Some((false, false)),
        Direction::Down => Some((false, true)),
        _ => None,
    }
}
