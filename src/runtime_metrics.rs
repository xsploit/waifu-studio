//! Opt-in main-schedule timing; not GPU, surface-acquire or display timing.
use bevy::prelude::Resource;
use std::time::{Duration, Instant};
const LIMIT: usize = 4096;

#[derive(Resource, Default)]
pub(super) struct FramePhases {
    gate: Option<Instant>,
    previous_last: Option<Instant>,
    sleep: Duration,
    sleeps: Vec<f64>,
    work: Vec<f64>,
    between: Vec<f64>,
}
#[derive(Default)]
pub(super) struct PhaseReport {
    pub sleep_ms: (f64, f64),
    pub work_ms: (f64, f64),
    pub between_ms: (f64, f64),
    pub sleep_samples: usize,
    pub work_samples: usize,
    pub between_samples: usize,
}
fn push(samples: &mut Vec<f64>, duration: Duration) {
    if samples.len() < LIMIT {
        samples.push(duration.as_secs_f64() * 1000.);
    }
}
fn percentiles(samples: &mut [f64]) -> (f64, f64) {
    if samples.is_empty() {
        return (0., 0.);
    }
    samples.sort_unstable_by(f64::total_cmp);
    let at = |p: f64| samples[((samples.len() - 1) as f64 * p).ceil() as usize];
    (at(0.5), at(0.99))
}
impl FramePhases {
    pub fn begin(&mut self, now: Instant) {
        if let Some(last) = self.previous_last
            && let Some(elapsed) = now.checked_duration_since(last)
        {
            push(&mut self.between, elapsed);
        }
        self.gate = Some(now);
        self.sleep = Duration::ZERO;
    }
    pub fn slept(&mut self, duration: Duration) {
        if self.gate.is_some() {
            self.sleep = self.sleep.saturating_add(duration);
        }
    }
    pub fn end(&mut self, now: Instant) {
        let Some(gate) = self.gate.take() else { return };
        if let Some(elapsed) = now.checked_duration_since(gate) {
            push(&mut self.sleeps, self.sleep);
            push(&mut self.work, elapsed.saturating_sub(self.sleep));
            self.previous_last = Some(now);
        }
    }
    pub fn clear_samples(&mut self) {
        self.sleeps.clear();
        self.work.clear();
        self.between.clear();
    }
    pub fn take_window(&mut self) -> PhaseReport {
        let report = PhaseReport {
            sleep_ms: percentiles(&mut self.sleeps),
            work_ms: percentiles(&mut self.work),
            between_ms: percentiles(&mut self.between),
            sleep_samples: self.sleeps.len(),
            work_samples: self.work.len(),
            between_samples: self.between.len(),
        };
        self.clear_samples();
        report
    }
}
/// Shared between main and render worlds only when metrics are enabled.
/// Brackets include scheduler gaps and concurrent work: not isolated GPU calls.
#[derive(Resource, Clone, Default)]
pub(super) struct RenderPhases(std::sync::Arc<std::sync::Mutex<RenderSamples>>);
#[derive(Default)]
struct RenderSamples {
    prepare_start: Option<Instant>,
    render_start: Option<Instant>,
    prepare: Vec<f64>,
    render: Vec<f64>,
    mesh_slab_bytes: [u64; 3],
    mesh_allocations: [usize; 3],
}
#[derive(Clone, Copy)]
enum Stage {
    Prepare,
    Render,
}
#[derive(Default)]
pub(super) struct RenderReport {
    pub prepare_ms: (f64, f64),
    pub render_ms: (f64, f64),
    pub prepare_samples: usize,
    pub render_samples: usize,
    pub mesh_slab_bytes: [u64; 3],
    pub mesh_allocations: [usize; 3],
}
impl RenderPhases {
    fn begin(&self, stage: Stage, now: Instant) {
        let mut sample = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match stage {
            Stage::Prepare => sample.prepare_start = Some(now),
            Stage::Render => sample.render_start = Some(now),
        }
    }
    fn end(&self, stage: Stage, now: Instant) {
        let mut sample = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let start = match stage {
            Stage::Prepare => sample.prepare_start.take(),
            Stage::Render => sample.render_start.take(),
        };
        if let Some(start) = start
            && let Some(duration) = now.checked_duration_since(start)
        {
            match stage {
                Stage::Prepare => push(&mut sample.prepare, duration),
                Stage::Render => push(&mut sample.render, duration),
            }
        }
    }
    pub fn take_window(&self) -> RenderReport {
        let mut sample = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let report = RenderReport {
            prepare_ms: percentiles(&mut sample.prepare),
            render_ms: percentiles(&mut sample.render),
            prepare_samples: sample.prepare.len(),
            render_samples: sample.render.len(),
            mesh_slab_bytes: sample.mesh_slab_bytes,
            mesh_allocations: sample.mesh_allocations,
        };
        sample.prepare.clear();
        sample.render.clear();
        report
    }
}
fn prepare_begin(probe: bevy::prelude::Res<RenderPhases>) {
    probe.begin(Stage::Prepare, Instant::now());
}
fn prepare_end(probe: bevy::prelude::Res<RenderPhases>) {
    probe.end(Stage::Prepare, Instant::now());
}
fn render_begin(probe: bevy::prelude::Res<RenderPhases>) {
    probe.begin(Stage::Render, Instant::now());
}
fn render_end(probe: bevy::prelude::Res<RenderPhases>) {
    probe.end(Stage::Render, Instant::now());
}
fn mesh_memory_probe(
    probe: bevy::prelude::Res<RenderPhases>,
    allocator: bevy::prelude::Res<bevy::render::mesh::allocator::MeshAllocator>,
    mut previous: bevy::prelude::Local<Option<Instant>>,
) {
    use bevy::render::mesh::allocator::ElementClass;
    let now = Instant::now();
    if previous.is_some_and(|at| now.duration_since(at) < Duration::from_secs(2)) {
        return;
    }
    *previous = Some(now);
    let index = |class| match class {
        ElementClass::Vertex => 0,
        ElementClass::Index => 1,
        ElementClass::MorphTarget => 2,
    };
    let mut bytes = [0; 3];
    let mut counts = [0; 3];
    for slab in allocator.slabs.values() {
        bytes[index(slab.element_class())] += slab.buffer_size();
    }
    for key in allocator.key_to_slab.keys() {
        counts[index(key.class)] += 1;
    }
    let mut samples = probe
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    samples.mesh_slab_bytes = bytes;
    samples.mesh_allocations = counts;
}
/// Register observers, preserving the existing renderer systems and resources.
pub(super) fn install_render_probes(app: &mut bevy::prelude::App) {
    use bevy::{
        prelude::*,
        render::{
            Render, RenderApp, RenderSystems, renderer::render_system,
            view::window::prepare_windows,
        },
    };
    let probe = RenderPhases::default();
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .insert_resource(probe.clone())
            .add_systems(
                Render,
                (
                    prepare_begin.before(prepare_windows),
                    prepare_end.after(prepare_windows),
                )
                    .in_set(RenderSystems::PrepareViews),
            )
            .add_systems(
                Render,
                (
                    render_begin.before(render_system),
                    render_end.after(render_system),
                )
                    .in_set(RenderSystems::Render),
            )
            .add_systems(Render, mesh_memory_probe.in_set(RenderSystems::Cleanup));
        app.insert_resource(probe);
    }
}
#[cfg(test)]
mod tests {

    use super::*;
    #[test]
    fn cloned_world_probes_share_events_and_window_drain_preserves_inflight_start() {
        let main = RenderPhases::default();
        let render = main.clone();
        let t = Instant::now();
        render.begin(Stage::Prepare, t);
        assert_eq!(main.take_window().prepare_samples, 0);
        render.end(Stage::Prepare, t + Duration::from_millis(36));
        let r = main.take_window();
        assert_eq!(r.prepare_ms, (36., 36.));
        assert_eq!(r.prepare_samples, 1);
        assert_eq!(r.render_samples, 0);
        assert_eq!(render.take_window().prepare_samples, 0);
    }
    #[test]
    fn render_brackets_are_independent_bounded_and_ignore_duplicate_end() {
        let p = RenderPhases::default();
        let t = Instant::now();
        p.end(Stage::Render, t);
        for i in 0..LIMIT + 8 {
            let at = t + Duration::from_millis(i as u64 * 10);
            p.begin(Stage::Render, at);
            p.end(Stage::Render, at + Duration::from_millis(2));
            p.end(Stage::Render, at + Duration::from_millis(3));
        }
        let r = p.take_window();
        assert_eq!(r.render_samples, LIMIT);
        assert_eq!(r.render_ms, (2., 2.));
        assert_eq!(r.prepare_samples, 0);
    }

    #[test]
    fn separates_actual_sleep_from_sampled_work_and_preserves_between_window_anchor() {
        let zero = Instant::now();
        let mut p = FramePhases::default();
        p.begin(zero);
        p.slept(Duration::from_millis(5));
        p.end(zero + Duration::from_millis(12));
        let r = p.take_window();
        assert_eq!(r.sleep_ms, (5., 5.));
        assert_eq!(r.work_ms, (7., 7.));
        assert_eq!(r.between_samples, 0);
        p.begin(zero + Duration::from_millis(30));
        p.end(zero + Duration::from_millis(40));
        let r = p.take_window();
        assert_eq!(r.sleep_ms, (0., 0.));
        assert_eq!(r.work_ms, (10., 10.));
        assert_eq!(r.between_ms, (18., 18.));
        assert_eq!(
            (r.sleep_samples, r.work_samples, r.between_samples),
            (1, 1, 1)
        );
    }
    #[test]
    fn missing_begin_and_duplicate_last_do_not_create_or_shift_samples() {
        let zero = Instant::now();
        let mut p = FramePhases::default();
        p.end(zero);
        p.slept(Duration::from_secs(1));
        assert_eq!(p.take_window().work_samples, 0);
        p.begin(zero);
        p.end(zero + Duration::from_millis(2));
        p.end(zero + Duration::from_millis(20));
        p.begin(zero + Duration::from_millis(25));
        p.end(zero + Duration::from_millis(28));
        let r = p.take_window();
        assert_eq!(r.work_samples, 2);
        assert_eq!(r.between_ms, (23., 23.));
        assert_eq!(r.sleep_ms, (0., 0.));
    }
    #[test]
    fn samples_are_bounded_and_nonmonotonic_clock_data_does_not_become_a_zero_frame() {
        let zero = Instant::now();
        let mut p = FramePhases::default();
        p.begin(zero + Duration::from_secs(1));
        p.end(zero);
        assert_eq!(p.take_window().work_samples, 0);
        for i in 0..LIMIT + 17 {
            let at = zero + Duration::from_millis(i as u64 * 10);
            p.begin(at);
            p.end(at + Duration::from_millis(1));
        }
        let r = p.take_window();
        assert_eq!(
            (r.sleep_samples, r.work_samples, r.between_samples),
            (LIMIT, LIMIT, LIMIT)
        );
        assert!(
            p.sleeps.capacity() <= LIMIT
                && p.work.capacity() <= LIMIT
                && p.between.capacity() <= LIMIT
        );
    }
}
