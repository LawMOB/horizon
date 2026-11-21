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
    pub fn allowance(
        &mut self,
        uploaded_total: u64,
        entity_pressure_bytes: u64,
    ) -> Allowance {
        // Section batches submitted before this frame are included in the delta.
        // Entity pressure is per-frame: it estimates both staging transfers and
        // GPU-pose output traffic, and only reduces optional arena relocation.
        let pressure = uploaded_total
            .saturating_sub(self.uploaded)
            .saturating_add(entity_pressure_bytes);
        self.uploaded = uploaded_total;
        let burst = pressure >= UPLOAD_BURST;
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
                self.copy_budget.saturating_sub(pressure).max(MIN_COPY)
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

/// Estimate the dynamic entity path's per-frame transfer and pose-output traffic.
/// Required visible geometry still runs; only optional arena maintenance uses this.
pub(super) fn entity_pressure_bytes(frame: &super::Frame<'_>) -> u64 {
    let vertex_count = [
        frame.entities.len(),
        frame.additive.len(),
        frame.hand.len(),
        frame.see_through.len(),
        frame.shadow_only.len(),
        frame.glowing.len(),
    ]
    .into_iter()
    .fold(0usize, usize::saturating_add) as u64;
    let vertices = vertex_count.saturating_mul(std::mem::size_of::<super::Vertex>() as u64);
    let skin = frame.skin.map_or(0, |skin| {
        let staged = [
            std::mem::size_of_val(skin.new_model.as_slice()),
            std::mem::size_of_val(skin.parts.as_slice()),
            std::mem::size_of_val(skin.jobs.as_slice()),
            std::mem::size_of_val(skin.outline_jobs.as_slice()),
        ]
        .into_iter()
        .map(|bytes| bytes as u64)
        .fold(0u64, u64::saturating_add);
        let posed = u64::from(skin.vertices.saturating_add(skin.outline_vertices))
            .saturating_mul(std::mem::size_of::<super::Vertex>() as u64);
        staged.saturating_add(posed)
    });
    vertices
        .saturating_add(skin)
        .saturating_add(frame.static_block_entity_stats.snapshot_bytes_built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spike_reduces_copy_work_without_bouncing_back_near_the_boundary() {
        let mut governor = Governor::default();
        let sample = Duration::from_millis;
        governor.observe(sample(14));
        assert_eq!(governor.allowance(0, 0).bytes, 4 * MIB);
        governor.observe(sample(14));
        assert_eq!(governor.allowance(0, 0).bytes, 2 * MIB);
        for _ in 0..120 {
            governor.observe(sample(10));
            assert_eq!(governor.allowance(0, 0).bytes, 2 * MIB);
        }
        for _ in 0..59 {
            governor.observe(sample(7));
            assert_eq!(governor.allowance(0, 0).bytes, 2 * MIB);
        }
        governor.observe(sample(7));
        assert_eq!(governor.allowance(0, 0).bytes, 3 * MIB);
    }

    #[test]
    fn uploads_take_priority_but_cannot_starve_reclamation() {
        let mut governor = Governor::default();
        for frame in 1..120 {
            let allowance = governor.allowance(frame * 3 * MIB, 0);
            assert_eq!(allowance.bytes, MIB / 2);
            assert!(!allowance.start);
        }
        assert!(governor.allowance(120 * 3 * MIB, 0).start);
        assert!(!governor.allowance(121 * 3 * MIB, 0).start);
        assert!(!governor.allowance(239 * 3 * MIB, 0).start);
        assert!(governor.allowance(240 * 3 * MIB, 0).start);
    }

    #[test]
    fn entity_pressure_defers_optional_reclamation() {
        let mut governor = Governor::default();
        let allowance = governor.allowance(0, 3 * MIB);
        assert_eq!(allowance.bytes, MIN_COPY);
        assert!(!allowance.start);

        // Required entity work still runs in the renderer. With its burst gone,
        // the maintenance budget recovers immediately.
        let allowance = governor.allowance(0, 0);
        assert_eq!(allowance.bytes, ROOMY_COPY);
        assert!(allowance.start);
    }
}
