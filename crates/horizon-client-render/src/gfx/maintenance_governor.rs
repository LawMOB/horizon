//! Budgets relocation of resident meshes on a shared-memory renderer.
//! New meshes and the frame's required work never go through this controller.

use std::time::Duration;

const MIB: u64 = 1 << 20;
const MIN_COPY: u64 = MIB / 2;
const ROOMY_COPY: u64 = 4 * MIB;
const TIGHT_COPY: u64 = MIB / 2;
const RECOVERY_STEP: u64 = MIB;
const UPLOAD_BURST: u64 = 2 * MIB;
const MAX_DEFER_FRAMES: u8 = 120;

pub(super) struct Allowance {
    pub bytes: u64,
    pub start: bool,
}

pub(super) struct Governor {
    copy_budget: u64,
    slow_frames: u8,
    healthy_frames: u8,
    deferred: u8,
    uploaded: u64,
}

impl Default for Governor {
    fn default() -> Self {
        Self {
            copy_budget: ROOMY_COPY,
            slow_frames: 0,
            healthy_frames: 0,
            deferred: 0,
            uploaded: 0,
        }
    }
}

impl Governor {
    pub fn allowance(&mut self, uploaded_total: u64) -> Allowance {
        // Section batches submitted before this frame are included in the delta.
        let uploaded = uploaded_total.saturating_sub(self.uploaded);
        self.uploaded = uploaded_total;
        let burst = uploaded >= UPLOAD_BURST;
        self.deferred = if burst { self.deferred + 1 } else { 0 };
        // An arena with sustained uploads still gets a chance to reclaim space.
        let start = !burst || self.deferred == MAX_DEFER_FRAMES;
        if start {
            self.deferred = 0;
        }
        Allowance {
            bytes: if burst {
                MIN_COPY
            } else {
                self.copy_budget.saturating_sub(uploaded).max(MIN_COPY)
            },
            start,
        }
    }

    pub fn observe(&mut self, work: Duration) {
        // CPU render time excludes VSync and GPU waits; reserve margin within
        // iPhone 11's 16.7 ms refresh interval.
        if work >= Duration::from_millis(12) {
            self.slow_frames += 1;
            self.healthy_frames = 0;
            if self.slow_frames == 2 {
                self.copy_budget = (self.copy_budget / 2).max(TIGHT_COPY);
                self.slow_frames = 0;
            }
        } else if work <= Duration::from_millis(8) {
            self.slow_frames = 0;
            self.healthy_frames += 1;
            if self.healthy_frames == 60 {
                self.copy_budget = (self.copy_budget + RECOVERY_STEP).min(ROOMY_COPY);
                self.healthy_frames = 0;
            }
        } else {
            self.slow_frames = 0;
            self.healthy_frames = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spike_reduces_copy_work_without_bouncing_back_near_the_boundary() {
        let mut governor = Governor::default();
        let sample = Duration::from_millis;
        governor.observe(sample(14));
        assert_eq!(governor.allowance(0).bytes, 4 * MIB);
        governor.observe(sample(14));
        assert_eq!(governor.allowance(0).bytes, 2 * MIB);
        for _ in 0..120 {
            governor.observe(sample(10));
            assert_eq!(governor.allowance(0).bytes, 2 * MIB);
        }
        for _ in 0..59 {
            governor.observe(sample(7));
            assert_eq!(governor.allowance(0).bytes, 2 * MIB);
        }
        governor.observe(sample(7));
        assert_eq!(governor.allowance(0).bytes, 3 * MIB);
    }

    #[test]
    fn uploads_take_priority_but_cannot_starve_reclamation() {
        let mut governor = Governor::default();
        for frame in 1..120 {
            let allowance = governor.allowance(frame * 3 * MIB);
            assert_eq!(allowance.bytes, MIB / 2);
            assert!(!allowance.start);
        }
        assert!(governor.allowance(120 * 3 * MIB).start);
        assert!(!governor.allowance(121 * 3 * MIB).start);
        assert!(!governor.allowance(239 * 3 * MIB).start);
        assert!(governor.allowance(240 * 3 * MIB).start);
    }
}
