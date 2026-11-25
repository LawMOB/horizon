//! Spike-triggered frame diagnostics. Normal frames update fixed-size samples only;
//! formatting and filesystem access happen below the spike gate.
use std::{
    cell::Cell,
    fs::{self, OpenOptions},
    io::Write,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const FRAME_SPIKE_MS: f64 = 17.0;
const LOG_INTERVAL: Duration = Duration::from_millis(500);
const LOG_LIMIT: u64 = 1 << 20;
const SERVER_SAMPLE_MAX_AGE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Default)]
struct FrameSample {
    client_tick: Duration,
    chunk_upload: Duration,
    entity_prep: Duration,
    mob_geometry: Duration,
    block_entities: Duration,
    shadow_casters: Duration,
    terrain_upload_bytes: u64,
    entity_vertices: usize,
    tracked_entities: usize,
    mob_count: usize,
    nearby_mobs: usize,
    rendered_mobs: usize,
    block_entity_count: usize,
    shadow_caster_count: usize,
}

#[derive(Clone, Copy, Default)]
struct ServerTickParts {
    mobs_ns: u64,
    collision_push_ns: u64,
    grid_ns: u64,
    save_ns: u64,
}

#[derive(Clone, Copy, Default)]
struct ServerSnapshot {
    total_ns: u64,
    mobs_ns: u64,
    collision_push_ns: u64,
    grid_ns: u64,
    save_ns: u64,
    at_ns: u64,
}

thread_local! {
    static FRAME: Cell<FrameSample> = const { Cell::new(FrameSample {
        client_tick: Duration::ZERO,
        chunk_upload: Duration::ZERO,
        entity_prep: Duration::ZERO,
        mob_geometry: Duration::ZERO,
        block_entities: Duration::ZERO,
        shadow_casters: Duration::ZERO,
        terrain_upload_bytes: 0,
        entity_vertices: 0,
        tracked_entities: 0,
        mob_count: 0,
        nearby_mobs: 0,
        rendered_mobs: 0,
        block_entity_count: 0,
        shadow_caster_count: 0,
    }) };
    static SERVER_PARTS: Cell<ServerTickParts> = const { Cell::new(ServerTickParts {
        mobs_ns: 0,
        collision_push_ns: 0,
        grid_ns: 0,
        save_ns: 0,
    }) };
    static LAST_LOG: Cell<Option<Instant>> = const { Cell::new(None) };
}

static SERVER_SEQ: AtomicU64 = AtomicU64::new(0);
static SERVER_TOTAL_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_MOBS_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_COLLISION_PUSH_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_GRID_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_SAVE_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_AT_NS: AtomicU64 = AtomicU64::new(0);
static STARTED: OnceLock<Instant> = OnceLock::new();

#[inline]
fn clock_ns() -> u64 {
    STARTED
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn update_frame(update: impl FnOnce(&mut FrameSample)) {
    FRAME.with(|cell| {
        let mut sample = cell.get();
        update(&mut sample);
        cell.set(sample);
    });
}

/// Start a fixed-size frame sample on the render thread.
pub fn begin() {
    FRAME.with(|sample| sample.set(FrameSample::default()));
}

#[inline]
pub fn started() -> Instant {
    Instant::now()
}

pub fn client_tick(duration: Duration) {
    update_frame(|sample| sample.client_tick += duration);
}

pub fn chunk_upload(duration: Duration, bytes: u64) {
    update_frame(|sample| {
        sample.chunk_upload += duration;
        sample.terrain_upload_bytes = sample.terrain_upload_bytes.saturating_add(bytes);
    });
}

pub fn entity_prep(duration: Duration, vertices: usize) {
    update_frame(|sample| {
        sample.entity_prep += duration;
        sample.entity_vertices = vertices;
    });
}

pub fn mob_geometry(duration: Duration) {
    update_frame(|sample| sample.mob_geometry += duration);
}

pub fn mob_counts(
    total: usize,
    nearby: usize,
    rendered: usize,
    tracked_entities: usize,
) {
    update_frame(|sample| {
        sample.mob_count = total;
        sample.nearby_mobs = nearby;
        sample.rendered_mobs = rendered;
        sample.tracked_entities = tracked_entities;
    });
}

pub fn block_entities(duration: Duration, count: usize) {
    update_frame(|sample| {
        sample.block_entities += duration;
        sample.block_entity_count = count;
    });
}

pub fn shadow_casters(duration: Duration, count: usize) {
    update_frame(|sample| {
        sample.shadow_casters += duration;
        sample.shadow_caster_count = count;
    });
}

#[inline]
fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

pub fn server_mobs(duration: Duration) {
    SERVER_PARTS.with(|cell| {
        let mut parts = cell.get();
        parts.mobs_ns = parts.mobs_ns.saturating_add(duration_ns(duration));
        cell.set(parts);
    });
}

pub fn server_collision_push(duration: Duration) {
    SERVER_PARTS.with(|cell| {
        let mut parts = cell.get();
        parts.collision_push_ns = parts
            .collision_push_ns
            .saturating_add(duration_ns(duration));
        cell.set(parts);
    });
}

pub fn server_grid(duration: Duration) {
    SERVER_PARTS.with(|cell| {
        let mut parts = cell.get();
        parts.grid_ns = parts.grid_ns.saturating_add(duration_ns(duration));
        cell.set(parts);
    });
}
pub fn server_save(duration: Duration) {
    SERVER_PARTS.with(|cell| {
        let mut parts = cell.get();
        parts.save_ns = parts.save_ns.saturating_add(duration_ns(duration));
        cell.set(parts);
    });
}

pub fn begin_server_tick() {
    SERVER_PARTS.with(|parts| parts.set(ServerTickParts::default()));
}

pub fn finish_server_tick(duration: Duration) {
    let parts = SERVER_PARTS.with(Cell::get);
    SERVER_SEQ.fetch_add(1, Ordering::AcqRel);
    SERVER_TOTAL_NS.store(duration_ns(duration), Ordering::Relaxed);
    SERVER_MOBS_NS.store(parts.mobs_ns, Ordering::Relaxed);
    SERVER_COLLISION_PUSH_NS.store(parts.collision_push_ns, Ordering::Relaxed);
    SERVER_GRID_NS.store(parts.grid_ns, Ordering::Relaxed);
    SERVER_SAVE_NS.store(parts.save_ns, Ordering::Relaxed);
    SERVER_AT_NS.store(clock_ns(), Ordering::Relaxed);
    SERVER_SEQ.fetch_add(1, Ordering::Release);
}
fn server_snapshot() -> ServerSnapshot {
    for _ in 0..3 {
        let before = SERVER_SEQ.load(Ordering::Acquire);
        if before & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        let snapshot = ServerSnapshot {
            total_ns: SERVER_TOTAL_NS.load(Ordering::Relaxed),
            mobs_ns: SERVER_MOBS_NS.load(Ordering::Relaxed),
            collision_push_ns: SERVER_COLLISION_PUSH_NS.load(Ordering::Relaxed),
            grid_ns: SERVER_GRID_NS.load(Ordering::Relaxed),
            save_ns: SERVER_SAVE_NS.load(Ordering::Relaxed),
            at_ns: SERVER_AT_NS.load(Ordering::Relaxed),
        };
        if before == SERVER_SEQ.load(Ordering::Acquire) {
            return snapshot;
        }
    }
    ServerSnapshot::default()
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn permit_log(now: Instant) -> bool {
    LAST_LOG.with(|last| {
        let previous = last.get();
        if previous.is_some_and(|at| now.duration_since(at) < LOG_INTERVAL) {
            false
        } else {
            last.set(Some(now));
            true
        }
    })
}

fn append_line(line: &str) -> std::io::Result<()> {
    let dir = horizon_client::logging::player_dir().join("logs");
    fs::create_dir_all(&dir)?;
    let path = dir.join("frame_spikes.log");
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let bytes = line.as_bytes();
    if file.metadata()?.len().saturating_add(bytes.len() as u64) > LOG_LIMIT {
        file.set_len(0)?;
    }
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Log slow frames only. Renderer wait includes acquisition, queue back-pressure and present;
/// render CPU time is separate and is never interpreted as CPU work.
pub fn finish(
    frame_time: Duration,
    frame_cpu: Duration,
    render_cpu: Duration,
    render_wait: Duration,
    resident_chunks: usize,
    pending_uploads: usize,
) {
    if ms(frame_time) < FRAME_SPIKE_MS || !permit_log(Instant::now()) {
        return;
    }

    let sample = FRAME.with(Cell::get);
    let server = server_snapshot();
    let server_age_ns = clock_ns().saturating_sub(server.at_ns);
    let server_fresh = server.at_ns != 0 && server_age_ns <= duration_ns(SERVER_SAMPLE_MAX_AGE);
    let (
        server_tick_ms,
        server_mob_ms,
        collision_push_ms,
        server_grid_ms,
        server_save_ms,
        server_age_ms,
    ) = if server_fresh {
        (
            server.total_ns as f64 / 1_000_000.0,
            server.mobs_ns as f64 / 1_000_000.0,
            server.collision_push_ns as f64 / 1_000_000.0,
            server.grid_ns as f64 / 1_000_000.0,
            server.save_ns as f64 / 1_000_000.0,
            server_age_ns as f64 / 1_000_000.0,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
    };
    let frame_ms = ms(frame_time);
    let fps = 1000.0 / frame_ms.max(0.001);
    let line = format!(
        "Horizon frame spike frame_ms={frame_ms:.2} fps={fps:.1} frame_work_ms={:.2} client_tick_ms={:.2} render_cpu_ms={:.2} render_wait_ms={:.2} server_tick_ms={server_tick_ms:.2} server_sample_age_ms={server_age_ms:.1} mob_tick_ms={server_mob_ms:.2} collision_push_ms={collision_push_ms:.2} spatial_grid_ms={server_grid_ms:.2} server_save_ms={server_save_ms:.2} chunk_upload_phase_ms={:.2} terrain_upload_bytes={} entity_prep_ms={:.2} entity_vertices={} mob_geometry_ms={:.2} tracked_entities={} total_mobs={} nearby_mobs_32={} rendered_mobs={} block_entity_ms={:.2} visible_block_entities={} shadow_caster_ms={:.2} shadow_casters={} resident_chunks={resident_chunks} pending_uploads={pending_uploads}",
        ms(frame_cpu),
        ms(sample.client_tick),
        ms(render_cpu),
        ms(render_wait),
        ms(sample.chunk_upload),
        sample.terrain_upload_bytes,
        ms(sample.entity_prep),
        sample.entity_vertices,
        ms(sample.mob_geometry),
        sample.tracked_entities,
        sample.mob_count,
        sample.nearby_mobs,
        sample.rendered_mobs,
        ms(sample.block_entities),
        sample.block_entity_count,
        ms(sample.shadow_casters),
        sample.shadow_caster_count,
    );
    log::warn!("{line}");
    if let Err(error) = append_line(&line) {
        log::warn!("Horizon: couldn't append frame spike diagnostic: {error}");
    }
}
