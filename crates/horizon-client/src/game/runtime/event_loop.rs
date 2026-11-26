//! What the game asks of the loop that runs it.
//!
//! A window's game runs inside winit's event loop, and its methods used to take winit's
//! `ActiveEventLoop` only to stop the loop or to say when to wake next. A headless client
//! (#4951) has no display, so it cannot have that loop: it runs its own, and hands the same
//! methods this trait instead. winit's loop implements it, so a window's game is unchanged.

use std::time::{Duration, Instant};
use winit::event_loop::{ActiveEventLoop, ControlFlow};

/// A successful frame only permits polling until the next idle iteration. If
/// the compositor stops delivering redraws, the old result must not keep the
/// loop spinning indefinitely. Offscreen frames are driven by the loop itself.
pub(super) fn redraw_flow(frame_drawn: &mut bool, offscreen: bool, now: Instant) -> ControlFlow {
    let drew_frame = std::mem::take(frame_drawn);
    if offscreen || drew_frame {
        ControlFlow::Poll
    } else {
        ControlFlow::WaitUntil(now + Duration::from_millis(16))
    }
}

/// UIKit's redraw callback must return before Core Animation commits the frame.
/// Onscreen iOS frames run from idle, with the cap expressed as a wake-up rather
/// than a sleep inside that callback. Other rendering paths keep their limiter.
#[cfg(any(test, target_os = "ios"))]
pub(super) struct IosFramePacing {
    active: bool,
    occluded: bool,
    retry: Option<Instant>,
}

#[cfg(any(test, target_os = "ios"))]
impl Default for IosFramePacing {
    fn default() -> Self {
        Self {
            active: true,
            occluded: false,
            retry: None,
        }
    }
}

#[cfg(any(test, target_os = "ios"))]
impl IosFramePacing {
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        self.retry = None;
    }

    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        self.retry = None;
    }

    /// A failed acquire retries later; a slow successful frame gets no extra
    /// delay beyond its cap, whose deadline starts at the frame's beginning.
    pub fn drawn(&mut self, drawn: bool, now: Instant) {
        self.retry = (!drawn).then(|| now + Duration::from_millis(16));
    }

    pub fn flow(&self, previous: Instant, limit: i32, now: Instant) -> ControlFlow {
        if !self.active || self.occluded {
            return ControlFlow::Wait;
        }
        let cap = (limit < 260).then(|| {
            previous + Duration::from_nanos(1_000_000_000 / limit.max(1) as u64)
        });
        match cap.into_iter().chain(self.retry).max() {
            Some(deadline) if deadline > now => ControlFlow::WaitUntil(deadline),
            _ => ControlFlow::Poll,
        }
    }
}

pub(super) trait EventLoopCtl {
    /// Ends the loop after the current iteration (`ActiveEventLoop::exit`).
    fn exit(&self);
    /// When the loop wakes next (`ActiveEventLoop::set_control_flow`).
    fn set_control_flow(&self, flow: ControlFlow);
}

impl EventLoopCtl for ActiveEventLoop {
    fn exit(&self) {
        ActiveEventLoop::exit(self);
    }
    fn set_control_flow(&self, flow: ControlFlow) {
        ActiveEventLoop::set_control_flow(self, flow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_redraw_waits_and_a_new_frame_resumes_polling() {
        let now = Instant::now();
        let mut frame_drawn = true;
        assert_eq!(redraw_flow(&mut frame_drawn, false, now), ControlFlow::Poll);
        assert_eq!(
            redraw_flow(&mut frame_drawn, false, now),
            ControlFlow::WaitUntil(now + Duration::from_millis(16))
        );
        frame_drawn = true;
        assert_eq!(redraw_flow(&mut frame_drawn, false, now), ControlFlow::Poll);
    }

    #[test]
    fn offscreen_keeps_polling_without_a_presented_frame() {
        let mut frame_drawn = false;
        assert_eq!(
            redraw_flow(&mut frame_drawn, true, Instant::now()),
            ControlFlow::Poll
        );
    }

    #[test]
    fn ios_yields_until_the_cap_without_delaying_slow_frames() {
        let start = Instant::now();
        let mut pacing = IosFramePacing::default();
        let deadline = start + Duration::from_nanos(1_000_000_000 / 30);
        assert_eq!(
            pacing.flow(start, 30, start + Duration::from_millis(3)),
            ControlFlow::WaitUntil(deadline)
        );
        pacing.drawn(true, start + Duration::from_millis(25));
        assert_eq!(
            pacing.flow(start, 60, start + Duration::from_millis(25)),
            ControlFlow::Poll
        );
        pacing.drawn(false, start + Duration::from_millis(25));
        assert_eq!(
            pacing.flow(start, 260, start + Duration::from_millis(25)),
            ControlFlow::WaitUntil(start + Duration::from_millis(41))
        );
        pacing.set_active(false);
        assert_eq!(pacing.flow(start, 260, deadline), ControlFlow::Wait);
        pacing.set_occluded(true);
        pacing.set_active(true);
        assert_eq!(pacing.flow(start, 260, deadline), ControlFlow::Wait);
        pacing.set_occluded(false);
        assert_eq!(pacing.flow(start, 260, deadline), ControlFlow::Poll);
    }
}
