//! Spike-triggered frame diagnostics. Normal frames update fixed-size samples only;
//! formatting and filesystem access happen below the spike gate.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, SyncSender, TrySendError},
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use horizon_client::gfx::RenderWaitTrace;
use winit::window::WindowId;

const FRAME_SPIKE_MS: f64 = 17.0;
const LOG_INTERVAL: Duration = Duration::from_millis(500);
const LOG_LIMIT: u64 = 1 << 20;
const BURST_LIMIT: u64 = 2 << 20;
const BURST_TRIGGER_MS: f64 = 1_000.0 / 58.0;
const BURST_PRE_FRAMES: usize = 30;
const BURST_POST_FRAMES: usize = 90;
const BURST_COOLDOWN: Duration = Duration::from_secs(5);
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
struct FramePacingSample {
    frame_id: u64,
    frame_start_ns: u64,
    previous_work: Duration,
    previous_render_wait: Duration,
    previous_finished_frame_id: u64,
    previous_wait_trace: Option<RenderWaitTrace>,
    previous_frame_tail: Duration,
    redraw_delivery: Option<Duration>,
    request_to_callback: Option<Duration>,
    callback_to_start: Option<Duration>,
    callback: Option<RedrawCallback>,
}

#[derive(Clone, Copy)]
struct RedrawRequest {
    at_ns: u64,
    from_frame_id: u64,
    window: WindowId,
    count: u32,
    ambiguous: bool,
}

#[derive(Clone, Copy)]
struct RedrawCallback {
    at_ns: u64,
    request: Option<RedrawRequest>,
    paired: bool,
}

#[derive(Clone, Copy, Default)]
struct FramePacingState {
    frame_id: u64,
    previous_work: Duration,
    previous_render_wait: Duration,
    previous_finished_frame_id: u64,
    previous_wait_trace: Option<RenderWaitTrace>,
    request: Option<RedrawRequest>,
    callback: Option<RedrawCallback>,
    tail_started_at_ns: Option<u64>,
    previous_frame_tail_ns: u64,
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

#[derive(Clone, Copy, Default)]
struct FramePacingRecord {
    frame_id: u64,
    frame_start_ns: u64,
    frame_ms: f64,
    previous_work_ms: f64,
    previous_render_wait_ms: f64,
    previous_wait_land_ms: f64,
    previous_wait_queue_before_acquire_ms: f64,
    previous_wait_acquire_ms: f64,
    previous_wait_queue_before_present_ms: f64,
    previous_wait_present_ms: f64,
    previous_wait_present_call_ms: f64,
    previous_frame_tail_ms: f64,
    redraw_delivery_ms: f64,
    request_to_callback_ms: f64,
    callback_to_start_ms: f64,
    request_count: u32,
    redraw_pairing_valid: bool,
    previous_wait_valid: bool,
    current_work_ms: f64,
    current_client_tick_ms: f64,
    current_render_cpu_ms: f64,
    current_render_wait_ms: f64,
    current_wait_land_ms: f64,
    current_wait_queue_before_acquire_ms: f64,
    current_wait_acquire_ms: f64,
    current_wait_queue_before_present_ms: f64,
    current_wait_present_ms: f64,
    current_wait_present_call_ms: f64,
    current_wait_valid: bool,
    server_tick_ms: f64,
    mob_tick_ms: f64,
    collision_push_ms: f64,
    server_grid_ms: f64,
    server_save_ms: f64,
    server_sample_age_ms: f64,
    chunk_upload_ms: f64,
    terrain_upload_bytes: u64,
    entity_prep_ms: f64,
    mob_geometry_ms: f64,
    block_entity_ms: f64,
    shadow_caster_ms: f64,
    total_mobs: usize,
    nearby_mobs: usize,
    resident_chunks: usize,
    pending_uploads: usize,
}

struct FrameBurst {
    trigger_frame_id: u64,
    trigger_count: u32,
    peak_frame_ms: f64,
    records: Vec<FramePacingRecord>,
}

struct ActiveBurst {
    burst: FrameBurst,
    frames_remaining: usize,
}

#[derive(Default)]
struct BurstCapture {
    history: VecDeque<FramePacingRecord>,
    active: Option<ActiveBurst>,
    last_trigger_ns: Option<u64>,
}

impl BurstCapture {
    fn push(&mut self, record: FramePacingRecord, now_ns: u64) {
        let mut completed = None;
        if let Some(mut active) = self.active.take() {
            if record.frame_ms > BURST_TRIGGER_MS {
                active.burst.trigger_count = active.burst.trigger_count.saturating_add(1);
                active.burst.peak_frame_ms = active.burst.peak_frame_ms.max(record.frame_ms);
            }
            active.burst.records.push(record);
            active.frames_remaining = active.frames_remaining.saturating_sub(1);
            if active.frames_remaining == 0 {
                completed = Some(active.burst);
            } else {
                self.active = Some(active);
            }
        } else {
            let cooled_down = self.last_trigger_ns.is_none_or(|last| {
                now_ns.saturating_sub(last) >= duration_ns(BURST_COOLDOWN)
            });
            if record.resident_chunks > 0 && record.frame_ms > BURST_TRIGGER_MS && cooled_down {
                let mut records = Vec::with_capacity(
                    BURST_PRE_FRAMES + 1 + BURST_POST_FRAMES,
                );
                records.extend(self.history.iter().copied());
                records.push(record);
                self.active = Some(ActiveBurst {
                    burst: FrameBurst {
                        trigger_frame_id: record.frame_id,
                        trigger_count: 1,
                        peak_frame_ms: record.frame_ms,
                        records,
                    },
                    frames_remaining: BURST_POST_FRAMES,
                });
                self.last_trigger_ns = Some(now_ns);
            }
        }

        self.history.push_back(record);
        while self.history.len() > BURST_PRE_FRAMES {
            self.history.pop_front();
        }
        if let Some(burst) = completed {
            enqueue_burst(burst);
        }
    }
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
    static PACING_SAMPLE: Cell<Option<FramePacingSample>> = const { Cell::new(None) };
    static PACING: Cell<FramePacingState> = const { Cell::new(FramePacingState {
        frame_id: 0,
        previous_work: Duration::ZERO,
        previous_render_wait: Duration::ZERO,
        previous_finished_frame_id: 0,
        previous_wait_trace: None,
        request: None,
        callback: None,
        tail_started_at_ns: None,
        previous_frame_tail_ns: 0,
    }) };
    static BURST_CAPTURE: RefCell<BurstCapture> = RefCell::new(BurstCapture::default());
}

static SERVER_SEQ: AtomicU64 = AtomicU64::new(0);
static SERVER_TOTAL_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_MOBS_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_COLLISION_PUSH_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_GRID_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_SAVE_NS: AtomicU64 = AtomicU64::new(0);
static SERVER_AT_NS: AtomicU64 = AtomicU64::new(0);
static STARTED: OnceLock<Instant> = OnceLock::new();
static PACING_PROBE: OnceLock<bool> = OnceLock::new();
static BURST_WRITER: OnceLock<SyncSender<FrameBurst>> = OnceLock::new();

/// Called once after launch has selected the player folder and initialized logging.
/// Both switches are opt-in; reading the file and writing the session marker happen at startup.
pub fn init(config_dir: &Path) {
    PACING_PROBE.get_or_init(|| {
        let configured = fs::read_to_string(config_dir.join(horizon_client::graphics::FILE))
            .map(|text| horizon_client::graphics::Graphics::parse(&text).frame_pacing_trace)
            .unwrap_or(false);
        let environment = std::env::var("HORIZON_FRAME_PACING_TRACE")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true"));
        let enabled = environment || configured;
        if enabled {
            start_burst_writer();
        }
        horizon_client::gfx::set_frame_wait_trace(enabled);
        let source = if environment {
            "environment"
        } else if configured {
            "horizon-graphics.txt"
        } else {
            "default"
        };
        let line = format!(
            "Horizon frame pacing trace schema=4 enabled={enabled} source={source} env_enabled={environment} config_enabled={configured} pid={}",
            std::process::id(),
        );
        log::info!("{line}");
        if let Err(error) = append_line(&line) {
            log::warn!("Horizon: couldn't append frame pacing session marker: {error}");
        }
        enabled
    });
}

fn pacing_probe_enabled() -> bool {
    PACING_PROBE.get().copied().unwrap_or(false)
}

#[inline]
fn clock_ns() -> u64 {
    clock_at_ns(Instant::now())
}

fn clock_at_ns(at: Instant) -> u64 {
    at.saturating_duration_since(*STARTED.get_or_init(|| at))
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
pub fn begin(started: Instant) {
    FRAME.with(|sample| sample.set(FrameSample::default()));
    if !pacing_probe_enabled() {
        return;
    }
    let pacing = PACING.with(|cell| {
        let mut state = cell.get();
        state.frame_id = state.frame_id.saturating_add(1);
        let now_ns = clock_at_ns(started);
        let callback = state.callback.take();
        let paired = callback.filter(|c| c.paired);
        let request = paired.and_then(|c| c.request);
        let sample = FramePacingSample {
            frame_id: state.frame_id,
            frame_start_ns: now_ns,
            previous_work: state.previous_work,
            previous_render_wait: state.previous_render_wait,
            previous_finished_frame_id: state.previous_finished_frame_id,
            previous_wait_trace: state.previous_wait_trace,
            previous_frame_tail: Duration::from_nanos(state.previous_frame_tail_ns),
            redraw_delivery: request
                .map(|r| Duration::from_nanos(now_ns.saturating_sub(r.at_ns))),
            request_to_callback: paired.and_then(|c| {
                c.request
                    .map(|r| Duration::from_nanos(c.at_ns.saturating_sub(r.at_ns)))
            }),
            callback_to_start: callback
                .map(|c| Duration::from_nanos(now_ns.saturating_sub(c.at_ns))),
            callback,
        };
        state.previous_frame_tail_ns = 0;
        cell.set(state);
        sample
    });
    PACING_SAMPLE.with(|sample| sample.set(Some(pacing)));
}

/// Record the first pending redraw request and the time spent in the frame-loop tail.
pub fn redraw_requested(window: WindowId) {
    if !pacing_probe_enabled() {
        return;
    }
    PACING.with(|cell| {
        let mut state = cell.get();
        if let Some(request) = &mut state.request {
            request.count = request.count.saturating_add(1);
            request.ambiguous |=
                request.window != window || request.from_frame_id != state.frame_id;
        } else {
            let now_ns = clock_ns();
            state.request = Some(RedrawRequest {
                at_ns: now_ns,
                from_frame_id: state.frame_id,
                window,
                count: 1,
                ambiguous: false,
            });
            if let Some(started_at_ns) = state.tail_started_at_ns.take() {
                state.previous_frame_tail_ns = now_ns.saturating_sub(started_at_ns);
            }
        }
        cell.set(state);
    });
}

/// Native callback entry, before event routing, device recovery, fullscreen and IME work.
/// Consume requests here so a callback that skips `begin` cannot retain a stale pairing.
pub fn redraw_callback(window: WindowId) {
    if !pacing_probe_enabled() {
        return;
    }
    let at_ns = clock_ns();
    PACING.with(|cell| {
        let mut state = cell.get();
        let request = state.request.take();
        let paired = state.callback.is_none()
            && request.is_some_and(|r| {
                !r.ambiguous
                    && r.window == window
                    && r.from_frame_id == state.frame_id
                    && r.from_frame_id != 0
            });
        state.callback = Some(RedrawCallback {
            at_ns,
            request,
            paired,
        });
        cell.set(state);
    });
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

fn start_burst_writer() {
    if BURST_WRITER.get().is_some() {
        return;
    }
    let (sender, receiver) = mpsc::sync_channel::<FrameBurst>(1);
    let path = horizon_client::logging::player_dir()
        .join("logs")
        .join("frame_pacing_bursts.log");
    match thread::Builder::new()
        .name("Frame pacing trace writer".into())
        .spawn(move || {
            while let Ok(burst) = receiver.recv() {
                if let Err(error) = write_burst(&path, &burst) {
                    log::warn!("Horizon: couldn't write frame pacing burst: {error}");
                }
            }
        })
    {
        Ok(_) => {
            let _ = BURST_WRITER.set(sender);
        }
        Err(error) => log::warn!("Horizon: couldn't start frame pacing trace writer: {error}"),
    }
}

fn enqueue_burst(burst: FrameBurst) {
    let Some(writer) = BURST_WRITER.get() else {
        return;
    };
    match writer.try_send(burst) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            log::warn!("Horizon: frame pacing burst dropped because the trace writer is busy");
        }
        Err(TrySendError::Disconnected(_)) => {
            log::warn!("Horizon: frame pacing burst dropped because the trace writer stopped");
        }
    }
}

fn write_burst(path: &Path, burst: &FrameBurst) -> std::io::Result<()> {
    let mut output = format!(
        "Horizon frame pacing burst schema=1 trigger_frame_id={} trigger_count={} peak_interval_ms={:.2} frame_count={} pre_frames={} post_frames={}\n",
        burst.trigger_frame_id,
        burst.trigger_count,
        burst.peak_frame_ms,
        burst.records.len(),
        BURST_PRE_FRAMES,
        BURST_POST_FRAMES,
    );
    for record in &burst.records {
        output.push_str(&format!(
            "Horizon frame pacing sample frame_id={} interval_from_frame_id={} frame_start_monotonic_ns={} frame_ms={:.2} previous_work_ms={:.2} previous_render_wait_ms={:.2} prior_wait_land_ms={:.2} prior_wait_queue_before_acquire_ms={:.2} prior_wait_acquire_ms={:.2} prior_wait_queue_before_present_ms={:.2} prior_wait_present_block_ms={:.2} prior_wait_present_call_ms={:.2} prior_wait_valid={} previous_frame_tail_ms={:.2} redraw_delivery_ms={:.2} request_to_callback_ms={:.2} callback_to_start_ms={:.2} redraw_request_count={} redraw_pairing_valid={} current_work_ms={:.2} client_tick_ms={:.2} current_render_cpu_ms={:.2} current_render_wait_ms={:.2} current_wait_land_ms={:.2} current_wait_queue_before_acquire_ms={:.2} current_wait_acquire_ms={:.2} current_wait_queue_before_present_ms={:.2} current_wait_present_block_ms={:.2} current_wait_present_call_ms={:.2} current_wait_valid={} server_tick_ms={:.2} mob_tick_ms={:.2} collision_push_ms={:.2} server_grid_ms={:.2} server_save_ms={:.2} server_sample_age_ms={:.1} chunk_upload_ms={:.2} terrain_upload_bytes={} entity_prep_ms={:.2} mob_geometry_ms={:.2} block_entity_ms={:.2} shadow_caster_ms={:.2} total_mobs={} nearby_mobs_32={} resident_chunks={} pending_uploads={}\n",
            record.frame_id,
            record.frame_id.saturating_sub(1),
            record.frame_start_ns,
            record.frame_ms,
            record.previous_work_ms,
            record.previous_render_wait_ms,
            record.previous_wait_land_ms,
            record.previous_wait_queue_before_acquire_ms,
            record.previous_wait_acquire_ms,
            record.previous_wait_queue_before_present_ms,
            record.previous_wait_present_ms,
            record.previous_wait_present_call_ms,
            record.previous_wait_valid,
            record.previous_frame_tail_ms,
            record.redraw_delivery_ms,
            record.request_to_callback_ms,
            record.callback_to_start_ms,
            record.request_count,
            record.redraw_pairing_valid,
            record.current_work_ms,
            record.current_client_tick_ms,
            record.current_render_cpu_ms,
            record.current_render_wait_ms,
            record.current_wait_land_ms,
            record.current_wait_queue_before_acquire_ms,
            record.current_wait_acquire_ms,
            record.current_wait_queue_before_present_ms,
            record.current_wait_present_ms,
            record.current_wait_present_call_ms,
            record.current_wait_valid,
            record.server_tick_ms,
            record.mob_tick_ms,
            record.collision_push_ms,
            record.server_grid_ms,
            record.server_save_ms,
            record.server_sample_age_ms,
            record.chunk_upload_ms,
            record.terrain_upload_bytes,
            record.entity_prep_ms,
            record.mob_geometry_ms,
            record.block_entity_ms,
            record.shadow_caster_ms,
            record.total_mobs,
            record.nearby_mobs,
            record.resident_chunks,
            record.pending_uploads,
        ));
    }
    output.push_str(&format!(
        "Horizon frame pacing burst end trigger_frame_id={}\n",
        burst.trigger_frame_id,
    ));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if file
        .metadata()?
        .len()
        .saturating_add(output.len() as u64)
        > BURST_LIMIT
    {
        file.set_len(0)?;
    }
    file.write_all(output.as_bytes())
}
fn capture_burst_frame(
    frame_time: Duration,
    frame_cpu: Duration,
    render_cpu: Duration,
    render_wait: Duration,
    render_wait_trace: Option<RenderWaitTrace>,
    resident_chunks: usize,
    pending_uploads: usize,
) {
    let Some(pacing) = PACING_SAMPLE.with(Cell::get) else {
        return;
    };
    let frame = FRAME.with(Cell::get);
    let server = server_snapshot();
    let age_ns = clock_ns().saturating_sub(server.at_ns);
    let fresh = server.at_ns != 0 && age_ns <= duration_ns(SERVER_SAMPLE_MAX_AGE);
    let callback = pacing.callback;
    let request = callback.and_then(|callback| callback.request);
    let wait = render_wait_trace.unwrap_or_default();
    let previous_wait_valid = pacing.previous_wait_trace.is_some_and(|trace| trace.complete)
        && pacing.previous_finished_frame_id != 0
        && pacing.previous_finished_frame_id == pacing.frame_id.saturating_sub(1);
    let record = FramePacingRecord {
        frame_id: pacing.frame_id,
        frame_start_ns: pacing.frame_start_ns,
        frame_ms: ms(frame_time),
        previous_work_ms: ms(pacing.previous_work),
        previous_render_wait_ms: ms(pacing.previous_render_wait),
        previous_wait_land_ms: pacing.previous_wait_trace.map_or(0.0, |trace| ms(trace.land)),
        previous_wait_queue_before_acquire_ms: pacing.previous_wait_trace
            .map_or(0.0, |trace| ms(trace.queue_before_acquire)),
        previous_wait_acquire_ms: pacing.previous_wait_trace.map_or(0.0, |trace| ms(trace.acquire)),
        previous_wait_queue_before_present_ms: pacing.previous_wait_trace
            .map_or(0.0, |trace| ms(trace.queue_before_present)),
        previous_wait_present_ms: pacing.previous_wait_trace.map_or(0.0, |trace| ms(trace.present)),
        previous_wait_present_call_ms: pacing.previous_wait_trace
            .and_then(|trace| trace.present_call)
            .map_or(-1.0, ms),
        previous_frame_tail_ms: ms(pacing.previous_frame_tail),
        redraw_delivery_ms: pacing.redraw_delivery.map_or(-1.0, ms),
        request_to_callback_ms: pacing.request_to_callback.map_or(-1.0, ms),
        callback_to_start_ms: pacing.callback_to_start.map_or(-1.0, ms),
        request_count: request.map_or(0, |request| request.count),
        redraw_pairing_valid: callback.is_some_and(|callback| callback.paired),
        previous_wait_valid,
        current_work_ms: ms(frame_cpu),
        current_client_tick_ms: ms(frame.client_tick),
        current_render_cpu_ms: ms(render_cpu),
        current_render_wait_ms: ms(render_wait),
        current_wait_land_ms: ms(wait.land),
        current_wait_queue_before_acquire_ms: ms(wait.queue_before_acquire),
        current_wait_acquire_ms: ms(wait.acquire),
        current_wait_queue_before_present_ms: ms(wait.queue_before_present),
        current_wait_present_ms: ms(wait.present),
        current_wait_present_call_ms: wait.present_call.map_or(-1.0, ms),
        current_wait_valid: render_wait_trace.is_some_and(|trace| trace.complete),
        server_tick_ms: if fresh { server.total_ns as f64 / 1_000_000.0 } else { 0.0 },
        mob_tick_ms: if fresh { server.mobs_ns as f64 / 1_000_000.0 } else { 0.0 },
        collision_push_ms: if fresh {
            server.collision_push_ns as f64 / 1_000_000.0
        } else {
            0.0
        },
        server_grid_ms: if fresh { server.grid_ns as f64 / 1_000_000.0 } else { 0.0 },
        server_save_ms: if fresh { server.save_ns as f64 / 1_000_000.0 } else { 0.0 },
        server_sample_age_ms: if fresh { age_ns as f64 / 1_000_000.0 } else { 0.0 },
        chunk_upload_ms: ms(frame.chunk_upload),
        terrain_upload_bytes: frame.terrain_upload_bytes,
        entity_prep_ms: ms(frame.entity_prep),
        mob_geometry_ms: ms(frame.mob_geometry),
        block_entity_ms: ms(frame.block_entities),
        shadow_caster_ms: ms(frame.shadow_casters),
        total_mobs: frame.mob_count,
        nearby_mobs: frame.nearby_mobs,
        resident_chunks,
        pending_uploads,
    };
    BURST_CAPTURE.with(|capture| capture.borrow_mut().push(record, clock_ns()));
}

/// Log slow frames only. The interval ends at this frame's start; work/render durations
/// describe the current callback. Work is wall time minus known renderer waits, not CPU usage.
pub fn finish(
    frame_time: Duration,
    frame_cpu: Duration,
    render_cpu: Duration,
    render_wait: Duration,
    render_wait_trace: Option<RenderWaitTrace>,
    resident_chunks: usize,
    pending_uploads: usize,
) {
    if pacing_probe_enabled() {
        PACING.with(|cell| {
            let mut state = cell.get();
            state.previous_work = frame_cpu;
            state.previous_render_wait = render_wait;
            state.previous_finished_frame_id = state.frame_id;
            state.previous_wait_trace = render_wait_trace;
            state.tail_started_at_ns = Some(clock_ns());
            cell.set(state);
        });
    }
    if pacing_probe_enabled() {
        capture_burst_frame(
            frame_time,
            frame_cpu,
            render_cpu,
            render_wait,
            render_wait_trace,
            resident_chunks,
            pending_uploads,
        );
    }
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
    let line = if let Some(pacing) = PACING_SAMPLE.with(Cell::get) {
        let callback = pacing.callback;
        let request = callback.and_then(|c| c.request);
        let wait = pacing.previous_wait_trace.unwrap_or_default();
        let wait_valid = pacing.previous_wait_trace.is_some_and(|w| w.complete)
            && pacing.previous_finished_frame_id != 0
            && pacing.previous_finished_frame_id == pacing.frame_id.saturating_sub(1);
        format!(
            "{line} frame_id={} interval_from_frame_id={} frame_start_monotonic_ns={} prior_frame_work_ms={:.2} prior_render_wait_ms={:.2} prior_frame_tail_ms={:.2} redraw_delivery_ms={:.2} redraw_request_monotonic_ns={} redraw_request_from_frame_id={} redraw_request_count={} redraw_callback_monotonic_ns={} redraw_pairing_valid={} redraw_request_to_callback_ms={:.2} redraw_callback_to_start_ms={:.2} prior_render_wait_valid={} prior_wait_land_ms={:.2} prior_wait_queue_before_acquire_ms={:.2} prior_wait_acquire_ms={:.2} prior_wait_queue_before_present_ms={:.2} prior_wait_present_block_ms={:.2} prior_wait_surface={} prior_wait_frame_generation={} prior_wait_deferred_submit={} prior_wait_present_call_ms={:.2}",
            pacing.frame_id,
            pacing.frame_id.saturating_sub(1),
            pacing.frame_start_ns,
            ms(pacing.previous_work),
            ms(pacing.previous_render_wait),
            ms(pacing.previous_frame_tail),
            pacing.redraw_delivery.map_or(-1.0, ms),
            request.map_or(0, |r| r.at_ns),
            request.map_or(0, |r| r.from_frame_id),
            request.map_or(0, |r| r.count),
            callback.map_or(0, |c| c.at_ns),
            callback.is_some_and(|c| c.paired),
            pacing.request_to_callback.map_or(-1.0, ms),
            pacing.callback_to_start.map_or(-1.0, ms),
            wait_valid,
            ms(wait.land),
            ms(wait.queue_before_acquire),
            ms(wait.acquire),
            ms(wait.queue_before_present),
            ms(wait.present),
            wait.surface,
            wait.frame_generation,
            wait.deferred_submit,
            wait.present_call.map_or(-1.0, ms),
        )
    } else {
        line
    };
    log::warn!("{line}");
    if let Err(error) = append_line(&line) {
        log::warn!("Horizon: couldn't append frame spike diagnostic: {error}");
    }
}
