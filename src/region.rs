use std::cmp::{max, min};
use std::collections::BTreeSet;

use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::{RectangleKind, RegionAttributes};

/// Most rects a client's region is decomposed from, or into. Past either, the
/// region's bounding box stands in for it: decomposing is cubic in the rects
/// given, the result can be quadratic in them, and blur walks the result on
/// every frame it draws.
pub const MAX_REGION_RECTS: usize = 256;

/// Decompose a `RegionAttributes` (set of additive/subtractive rects) into a
/// non-overlapping list of rects covering the same area, or into its bounding
/// box past [`MAX_REGION_RECTS`].
pub fn region_to_non_overlapping_rects(
    region: &RegionAttributes,
    output: &mut Vec<Rectangle<i32, Logical>>,
) {
    output.clear();
    if region.rects.len() > MAX_REGION_RECTS {
        let added = region
            .rects
            .iter()
            .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
            .map(|(_, r)| *r);
        output.extend(bounding_box(added));
        return;
    }
    decompose(region, output);
    if output.len() > MAX_REGION_RECTS {
        let bounds = bounding_box(output.drain(..));
        output.extend(bounds);
    }
}

/// The smallest rect holding every non-empty rect of `rects`, in `i64` since
/// the sides are a client's to choose.
fn bounding_box(
    rects: impl Iterator<Item = Rectangle<i32, Logical>>,
) -> Option<Rectangle<i32, Logical>> {
    let (x1, y1, x2, y2) = rects
        .filter(|r| r.size.w > 0 && r.size.h > 0)
        .map(|r| {
            let (x, y) = (i64::from(r.loc.x), i64::from(r.loc.y));
            (x, y, x + i64::from(r.size.w), y + i64::from(r.size.h))
        })
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))?;
    let side = |len: i64| i32::try_from(len).unwrap_or(i32::MAX);
    Some(Rectangle::new(
        (x1 as i32, y1 as i32).into(),
        (side(x2 - x1), side(y2 - y1)).into(),
    ))
}

fn decompose(region: &RegionAttributes, output: &mut Vec<Rectangle<i32, Logical>>) {
    let ys = BTreeSet::from_iter(
        region
            .rects
            .iter()
            .flat_map(|(_, r)| [r.loc.y, r.loc.y + r.size.h]),
    );

    let mut ys = ys.into_iter();
    let Some(mut lo) = ys.next() else {
        return;
    };

    let mut spans = Vec::<(i32, i32)>::new();

    for hi in ys {
        spans.clear();

        'region: for (kind, r) in &region.rects {
            if hi <= r.loc.y || r.loc.y + r.size.h <= lo {
                continue;
            }

            let mut x1 = r.loc.x;
            let mut x2 = r.loc.x + r.size.w;
            if x1 == x2 {
                continue;
            }

            match *kind {
                RectangleKind::Add => {
                    for i in (0..spans.len()).rev() {
                        let (start, end) = spans[i];

                        if end < x1 {
                            spans.insert(i + 1, (x1, x2));
                            continue 'region;
                        }

                        if x2 < start {
                            continue;
                        }

                        spans.remove(i);
                        x1 = min(x1, start);
                        x2 = max(x2, end);
                    }

                    spans.insert(0, (x1, x2));
                }
                RectangleKind::Subtract => {
                    for i in (0..spans.len()).rev() {
                        let (start, end) = spans[i];

                        if end <= x1 {
                            continue 'region;
                        }

                        if x2 <= start {
                            continue;
                        }

                        spans.remove(i);
                        if x2 < end {
                            spans.insert(i, (x2, end));
                        }
                        if start < x1 {
                            spans.insert(i, (start, x1));
                        }
                    }
                }
            }
        }

        for (x1, x2) in spans.drain(..) {
            output.push(Rectangle::from_extremities((x1, lo), (x2, hi)));
        }

        lo = hi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::{Point, Size};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    fn rects_of(region: &RegionAttributes) -> Vec<Rectangle<i32, Logical>> {
        let mut out = Vec::new();
        region_to_non_overlapping_rects(region, &mut out);
        out
    }

    fn add(rects: impl IntoIterator<Item = Rectangle<i32, Logical>>) -> RegionAttributes {
        RegionAttributes {
            rects: rects.into_iter().map(|r| (RectangleKind::Add, r)).collect(),
        }
    }

    #[test]
    fn overlapping_rects_decompose_into_their_union() {
        let out = rects_of(&add([rect(0, 0, 10, 10), rect(5, 5, 10, 10)]));
        let area: i32 = out.iter().map(|r| r.size.w * r.size.h).sum();
        assert_eq!(area, 100 + 100 - 25);
        for (i, a) in out.iter().enumerate() {
            assert!(out[i + 1..].iter().all(|b| !a.overlaps(*b)));
        }
    }

    #[test]
    fn a_region_of_more_rects_than_the_cap_becomes_its_bounding_box() {
        let hole = (RectangleKind::Subtract, rect(40, 40, 20, 20));
        let mut rects = vec![(RectangleKind::Add, rect(0, 0, 100, 100))];
        rects.extend(std::iter::repeat_n(hole, MAX_REGION_RECTS));
        let region = RegionAttributes { rects };
        assert_eq!(rects_of(&region), [rect(0, 0, 100, 100)]);
    }

    /// Few rects can still decompose into many: staggered columns split every
    /// band, so the output is quadratic in the input.
    #[test]
    fn a_region_decomposing_into_more_rects_than_the_cap_becomes_its_bounding_box() {
        let n = 32;
        let region = add((0..n).map(|i| rect(4 * i, i, 2, 4 * n - 2 * i)));
        assert_eq!(rects_of(&region), [rect(0, 0, 4 * n - 2, 4 * n)]);
    }

    #[test]
    fn a_subtract_only_region_past_the_cap_is_empty() {
        let region = RegionAttributes {
            rects: (0..MAX_REGION_RECTS as i32 + 1)
                .map(|i| (RectangleKind::Subtract, rect(i, i, 1, 1)))
                .collect(),
        };
        assert_eq!(rects_of(&region), []);
    }
}
