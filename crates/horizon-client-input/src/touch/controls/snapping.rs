//! Editor-only snapping in GUI units. Candidates and guides stay on the stack;
//! the caller supplies the layout's existing, scaled rectangles.
use super::{Area, Control, Layout};

const SNAP_GAP: f32 = 4.0;

/// Editor preferences, independent of the saved HUD placement schema.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapSettings {
    pub enabled: bool,
    /// Maximum distance from the raw drag position, in logical GUI units.
    pub snap_threshold: f32,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            snap_threshold: 6.0,
        }
    }
}

/// One visual guide: its axis coordinate and its span on the other axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapGuide {
    pub at: f32,
    pub start: f32,
    pub end: f32,
}

/// At most one guide per axis; owned by the active drag, never the profile.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnapGuides {
    pub vertical: Option<SnapGuide>,
    pub horizontal: Option<SnapGuide>,
}

#[derive(Clone, Copy)]
struct Candidate {
    origin: f32,
    offset: f32,
    distance: f32,
    /// None means the full screen axis, otherwise the target's cross-axis span.
    span: Option<(f32, f32)>,
}

#[derive(Default)]
struct Axis {
    best: Option<Candidate>,
}

impl Axis {
    fn consider(
        &mut self,
        raw: f32,
        origin: f32,
        offset: f32,
        span: Option<(f32, f32)>,
        threshold: f32,
    ) {
        let distance = (origin - raw).abs();
        if distance <= threshold && self.best.is_none_or(|best| distance < best.distance) {
            self.best = Some(Candidate {
                origin,
                offset,
                distance,
                span,
            });
        }
    }

    fn target(
        &mut self,
        raw: f32,
        size: f32,
        other: f32,
        other_size: f32,
        span: (f32, f32),
        threshold: f32,
    ) {
        // Same near/far edge, centre, then adjacency in either direction.
        // Ties keep the first candidate for a stable result at equal distances.
        for (origin, offset) in [
            (other, 0.0),
            (other + other_size - size, size),
            (other + (other_size - size) / 2.0, size / 2.0),
            (other + other_size + SNAP_GAP, 0.0),
            (other - SNAP_GAP - size, size),
        ] {
            self.consider(raw, origin, offset, Some(span), threshold);
        }
    }

    fn position(&self, raw: f32) -> f32 {
        self.best.map_or(raw, |best| best.origin)
    }

    fn guide(&self, origin: f32, span: (f32, f32), screen_end: f32) -> Option<SnapGuide> {
        let best = self.best?;
        // Clamping may undo an alignment; do not advertise one that no longer holds.
        if (origin - best.origin).abs() > 0.001 {
            return None;
        }
        let (start, end) = best.span.map_or((0.0, screen_end), |target| {
            (span.0.min(target.0), span.1.max(target.1))
        });
        Some(SnapGuide {
            // For adjacency, mark the moving edge with the configured gap beside it.
            at: origin + best.offset,
            start,
            end,
        })
    }
}

/// Resolve each axis against the raw rectangle, then constrain the final result.
/// No candidate lists, guide lists or temporary layouts are allocated here.
pub(super) fn snap(
    layout: &Layout,
    control: Control,
    raw: Area,
    settings: SnapSettings,
) -> (Area, SnapGuides) {
    let mut x = Axis::default();
    let mut y = Axis::default();
    let threshold = settings.snap_threshold;
    if settings.enabled && threshold.is_finite() && threshold >= 0.0 {
        x.consider(
            raw.x,
            (layout.width - raw.w) / 2.0,
            raw.w / 2.0,
            None,
            threshold,
        );
        y.consider(
            raw.y,
            (layout.height - raw.h) / 2.0,
            raw.h / 2.0,
            None,
            threshold,
        );
        for item in &layout.items {
            if item.control == control || item.hidden {
                continue;
            }
            let other = layout.rendered_area(item);
            x.target(
                raw.x,
                raw.w,
                other.x,
                other.w,
                (other.y, other.y + other.h),
                threshold,
            );
            y.target(
                raw.y,
                raw.h,
                other.y,
                other.h,
                (other.x, other.x + other.w),
                threshold,
            );
        }
    }
    // The joystick's touch target is larger than its artwork. Its centre must also
    // satisfy Layout::new's screen clamp, or the next layout would move the artwork.
    let item = layout.item(control);
    let hit = item.area;
    let drawn = layout.rendered_area(item);
    let margin_x = ((hit.w - drawn.w) / 2.0).max(0.0);
    let margin_y = ((hit.h - drawn.h) / 2.0).max(0.0);
    let left = layout.safe.x.max(margin_x);
    let top = layout.safe.y.max(margin_y);
    let bounds = Area {
        x: left,
        y: top,
        w: ((layout.safe.x + layout.safe.w).min(layout.width - margin_x) - left).max(0.0),
        h: ((layout.safe.y + layout.safe.h).min(layout.height - margin_y) - top).max(0.0),
    };
    let area = Area {
        x: x.position(raw.x),
        y: y.position(raw.y),
        ..raw
    }
    .kept_in(bounds);
    let guides = SnapGuides {
        vertical: x.guide(area.x, (area.y, area.y + area.h), layout.height),
        horizontal: y.guide(area.y, (area.x, area.x + area.w), layout.width),
    };
    (area, guides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch::{Placement, TouchButton, TouchSettings};

    const MOVING: Control = Control::Button(TouchButton::Jump);
    const TARGET: Control = Control::Button(TouchButton::Use);

    fn layout() -> Layout {
        let mut settings = TouchSettings::default();
        for control in Control::all() {
            settings.set(control, Placement {
                hidden: true,
                ..Placement::default()
            });
        }
        settings.set(MOVING, Placement::default());
        settings.set(TARGET, Placement {
            centre: Some((130.0 / 400.0, 110.0 / 300.0)),
            size: 2.0,
            ..Placement::default()
        });
        Layout::new(
            400.0,
            300.0,
            Area {
                x: 0.0,
                y: 0.0,
                w: 400.0,
                h: 300.0,
            },
            &settings,
        )
    }

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }

    #[test]
    fn scaled_edges_centres_and_adjacent_gaps_snap_on_both_axes() {
        let layout = layout();
        close(layout.item(TARGET).area.w, 60.0);
        // Expected origins and the aligned point's offset within the dragged button.
        for (left, top, offset) in [
            (100.0, 80.0, 0.0),
            (130.0, 110.0, 30.0),
            (115.0, 95.0, 15.0),
            (164.0, 144.0, 0.0),
            (66.0, 46.0, 30.0),
        ] {
            let raw = Area {
                x: left + 1.0,
                y: top - 1.0,
                w: 30.0,
                h: 30.0,
            };
            let (area, guides) = snap(&layout, MOVING, raw, SnapSettings::default());
            close(area.x, left);
            close(area.y, top);
            let vertical = guides.vertical.unwrap();
            let horizontal = guides.horizontal.unwrap();
            close(vertical.at, left + offset);
            close(horizontal.at, top + offset);
            let target = layout.item(TARGET).area;
            assert!(vertical.start <= area.y && vertical.start <= target.y);
            assert!(vertical.end >= area.y + area.h && vertical.end >= target.y + target.h);
            assert!(horizontal.start <= area.x && horizontal.start <= target.x);
            assert!(horizontal.end >= area.x + area.w && horizontal.end >= target.x + target.w);
        }
        // Exactly six units still catches; just beyond six is free on both axes.
        let raw = Area {
            x: 106.0,
            y: 74.0,
            w: 30.0,
            h: 30.0,
        };
        let (area, _) = snap(&layout, MOVING, raw, SnapSettings::default());
        close(area.x, 100.0);
        close(area.y, 80.0);
        let raw = Area { x: 106.1, y: 73.9, ..raw };
        assert_eq!(
            snap(&layout, MOVING, raw, SnapSettings::default()),
            (raw, SnapGuides::default()),
        );
    }

    #[test]
    fn screen_axes_visibility_threshold_and_clamping_keep_guides_accurate() {
        let mut layout = layout();
        // The screen centre, even with an asymmetric safe area.
        layout.safe.x = 20.0;
        layout.safe.w = 380.0;
        let raw = Area {
            x: 188.0,
            y: 133.0,
            w: 30.0,
            h: 30.0,
        };
        let (area, guides) = snap(&layout, MOVING, raw, SnapSettings::default());
        close(area.x, 185.0);
        close(area.y, 135.0);
        assert_eq!(guides.vertical, Some(SnapGuide { at: 200.0, start: 0.0, end: 300.0 }));
        assert_eq!(guides.horizontal, Some(SnapGuide { at: 150.0, start: 0.0, end: 400.0 }));
        for settings in [
            SnapSettings { enabled: false, ..SnapSettings::default() },
            SnapSettings { snap_threshold: 1.0, ..SnapSettings::default() },
            SnapSettings { snap_threshold: f32::NAN, ..SnapSettings::default() },
        ] {
            assert_eq!(snap(&layout, MOVING, raw, settings), (raw, SnapGuides::default()));
        }
        // Final clamping breaks only the X alignment, leaving the Y guide valid.
        layout.safe.x = 105.0;
        layout.safe.w = 295.0;
        let (area, guides) = snap(
            &layout,
            MOVING,
            Area { x: 101.0, y: 79.0, ..raw },
            SnapSettings::default(),
        );
        close(area.x, 105.0);
        close(area.y, 80.0);
        assert!(guides.vertical.is_none());
        assert!(guides.horizontal.is_some());
        // Neither a hidden neighbour nor the button's own previous position is a target.
        layout.safe.x = 0.0;
        layout.safe.w = 400.0;
        let target = layout.items.iter_mut().find(|i| i.control == TARGET).unwrap();
        target.hidden = true;
        target.area.x = 364.0;
        target.area.y = 240.0;
        let raw = Area { x: 364.0, y: 240.0, ..raw };
        assert_eq!(
            snap(&layout, MOVING, raw, SnapSettings::default()),
            (raw, SnapGuides::default()),
        );
    }

    #[test]
    fn closest_candidate_wins_and_joystick_uses_artwork_bounds() {
        let mut layout = layout();
        let target = layout.items.iter_mut().find(|i| i.control == TARGET).unwrap();
        target.area.x = 101.0;
        let raw = Area {
            x: 103.0,
            y: 200.0,
            w: 30.0,
            h: 30.0,
        };
        // A smaller visible neighbour is closer than the first target.
        let other = layout.items.iter_mut()
            .find(|i| i.control == Control::Button(TouchButton::Chat)).unwrap();
        other.hidden = false;
        other.area = Area { x: 104.0, y: 80.0, w: 22.0, h: 22.0 };
        let (area, _) = snap(&layout, MOVING, raw, SnapSettings::default());
        close(area.x, 104.0);
        let stick = layout.items.iter_mut().find(|i| i.control == Control::Stick).unwrap();
        stick.hidden = false;
        let drawn = layout.rendered_area(layout.item(Control::Stick));
        close(drawn.w, 52.0);
        let raw = Area { x: drawn.x + drawn.w + SNAP_GAP + 1.0, y: 200.0, ..raw };
        let (area, _) = snap(&layout, MOVING, raw, SnapSettings::default());
        close(area.x, drawn.x + drawn.w + SNAP_GAP);
        // Moving the stick itself cannot make Layout::new's hitbox clamp shift it later.
        let (area, guides) = snap(
            &layout,
            Control::Stick,
            Area { x: -10.0, y: -10.0, ..drawn },
            SnapSettings::default(),
        );
        close(area.x, 26.0);
        close(area.y, 26.0);
        assert_eq!(guides, SnapGuides::default());
    }
}
