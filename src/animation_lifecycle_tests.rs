//! CPU-only logical playback lifecycle; no rendered-motion claims.
use super::*;
use crate::{
    extension_lifecycle::{LifecycleEvent, LifecycleJournal},
    extension_protocol::{MotionOutcome, MotionPlaybackSource},
    extension_runtime::ExtensionRuntime,
};
use std::time::Duration;
const SAMPLE: &str = "HIERARCHY ROOT hips { OFFSET 0 0 0 CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation JOINT leftUpperArm { OFFSET 1 1 0 CHANNELS 3 Zrotation Xrotation Yrotation End Site { OFFSET 1 0 0 } } } MOTION Frames: 2 Frame Time: 0.1 0 0 0 0 0 0 0 0 0 100 200 300 90 0 0 0 0 0";
fn clip() -> Arc<BvhClip> {
    Arc::new(parse_bvh(SAMPLE).unwrap())
}
fn terminals(source: &MotionObservations) -> Vec<LifecycleEvent> {
    let (events, lost) = source.take_terminals();
    assert_eq!(lost, 0);
    events.into_iter().map(|(e, _)| e).collect()
}
fn terminal(event: &LifecycleEvent) -> (uuid::Uuid, MotionOutcome) {
    let LifecycleEvent::MotionPlaybackEnded {
        playback_id,
        outcome,
        ..
    } = event
    else {
        panic!("not a motion terminal")
    };
    (*playback_id, *outcome)
}
fn deferred(
    state: &mut AnimationState,
    one_shot: bool,
    blend: bool,
) -> mpsc::SyncSender<ImportResult> {
    deferred_attributed(state, one_shot, blend, MotionSourceSnapshot::default())
}
fn deferred_attributed(
    state: &mut AnimationState,
    one_shot: bool,
    blend: bool,
    snapshot: MotionSourceSnapshot,
) -> mpsc::SyncSender<ImportResult> {
    let (tx, rx) = mpsc::sync_channel(1);
    state.begin_import("local.bvh".into(), rx);
    state.import_one_shot = one_shot;
    state.import_blend = blend;
    state.motion_import = Some(state.motion_observations.admit(snapshot));
    tx
}
fn delivery_app() -> App {
    let mut app = App::new();
    app.init_resource::<AnimationState>()
        .add_systems(Update, poll_import);
    app
}
#[test]
fn generic_import_activation_preserves_identity_all_three_paths_and_first_terminal() {
    for (one_shot, blend) in [(false, false), (false, true), (true, false)] {
        let mut app = delivery_app();
        let source = app
            .world()
            .resource::<AnimationState>()
            .motion_observations();
        let tx = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            deferred(&mut state, one_shot, blend)
        };
        let identity = {
            let state = app.world().resource::<AnimationState>();
            state.motion_import.as_ref().unwrap().id_for_test()
        };
        tx.send(Ok(("local.bvh".into(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        assert!(
            terminals(&source).is_empty(),
            "activation cancelled the admitted request"
        );
        app.world_mut().resource_mut::<AnimationState>().stop();
        let events = terminals(&source);
        assert_eq!(events.len(), 1);
        assert_eq!(terminal(&events[0]), (identity, MotionOutcome::Cancelled));
        app.world_mut().resource_mut::<AnimationState>().stop();
        assert!(terminals(&source).is_empty());
    }
}
#[test]
fn generic_import_failure_disconnect_rejection_and_late_completion() {
    for disconnected in [false, true] {
        let mut app = delivery_app();
        let source = app
            .world()
            .resource::<AnimationState>()
            .motion_observations();
        let tx = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            deferred(&mut state, false, false)
        };
        if !disconnected {
            tx.send(Err("private local path/error must stay private".into()))
                .unwrap();
        }
        drop(tx);
        app.update();
        let events = terminals(&source);
        assert_eq!(events.len(), 1);
        assert_eq!(terminal(&events[0]).1, MotionOutcome::Failed);
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        assert!(state.load_path("invalid.txt").is_err());
        assert!(terminals(&source).is_empty());
    }
    let mut app = delivery_app();
    let source = app
        .world()
        .resource::<AnimationState>()
        .motion_observations();
    let tx = {
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        deferred(&mut state, false, false)
    };
    app.world_mut().resource_mut::<AnimationState>().stop();
    assert_eq!(terminals(&source).len(), 1);
    tx.send(Ok(("local.bvh".into(), parse_bvh(SAMPLE).unwrap())))
        .unwrap();
    app.update();
    assert!(!app.world().resource::<AnimationState>().playing);
    assert!(terminals(&source).is_empty());
}
#[test]
fn generic_same_file_supersession_drop_and_replacement_survive_outside_state() {
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    for _ in 0..3 {
        state.select_clip(clip(), Some("same.bvh".into()));
    }
    drop(state);
    let events = terminals(&source);
    assert_eq!(events.len(), 3);
    let ids: HashSet<_> = events.iter().map(|e| terminal(e).0).collect();
    assert_eq!(ids.len(), 3);
    assert!(
        events
            .iter()
            .all(|e| terminal(e).1 == MotionOutcome::Cancelled)
    );
    let mut replacement = AnimationState::default();
    replacement.select_clip(clip(), None);
    source.attach(&mut replacement);
    drop(replacement);
    assert_eq!(terminals(&source).len(), 1);
}
#[test]
fn generic_desktop_park_restore_cancels_old_and_mints_fresh_without_changing_time_or_manual_flags()
{
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    state.select_clip(clip(), Some("ordinary.bvh".into()));
    state.take_manual_control();
    state.play();
    state.position = 0.13;
    state.speed = 1.5;
    let old = state.motion_playback.as_ref().unwrap().id_for_test();
    let previous = state.park_for_desktop();
    state.select_clip(clip(), Some("desktop.bvh".into()));
    state.restore_parked(previous);
    assert_eq!(state.position, 0.13);
    assert_eq!(state.speed, 1.5);
    assert!(state.manual_controlled() && state.playing);
    let new = state.motion_playback.as_ref().unwrap().id_for_test();
    assert_ne!(old, new);
    state.stop();
    let events = terminals(&source);
    assert_eq!(events.iter().filter(|e| terminal(e).0 == old).count(), 1);
    assert_eq!(events.iter().filter(|e| terminal(e).0 == new).count(), 1);
}
#[test]
fn generic_scrub_does_not_admit_synthetic_play_pause_and_revocation_preserves_other_import() {
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    state.select_clip(clip(), Some("old.bvh".into()));
    state.stop();
    terminals(&source);
    state.position = 0.05;
    state.show_scrub_pose();
    assert!(state.is_posing() && !state.playing);
    drop(state);
    assert!(terminals(&source).is_empty());
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    state.select_clip(clip(), Some("old.bvh".into()));
    let _tx = deferred(&mut state, false, false);
    terminals(&source);
    state.revoke_paths(&["old.bvh".into()]);
    assert!(
        terminals(&source).is_empty(),
        "unrelated import cancelled by revocation"
    );
    drop(state);
    assert_eq!(terminals(&source).len(), 1);
}
#[test]
fn generic_nonloop_completes_once_loops_do_not_and_no_avatar_does_not_advance() {
    use bevy::time::TimeUpdateStrategy;
    for mode in 0..3 {
        let looping = mode == 1;
        let reaction = mode == 2;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin);
        let root = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn((HumanBoneName::Hips, Transform::default(), ChildOf(root)));
        {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            if reaction {
                state.select_one_shot(clip(), None);
            } else {
                state.select_clip(clip(), None);
                state.looping = looping;
            }
        }
        let source = app
            .world()
            .resource::<AnimationState>()
            .motion_observations();
        app.update();
        assert!(terminals(&source).is_empty());
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        for _ in 0..10 {
            app.update();
        }
        let events = terminals(&source);
        if looping {
            assert!(events.is_empty());
        } else {
            assert_eq!(events.len(), 1);
            assert_eq!(terminal(&events[0]).1, MotionOutcome::Completed);
        }
    }
}
#[test]
fn generic_observation_bounds_surface_a_gap_without_fabricated_terminals() {
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    for _ in 0..MAX_MOTION_RECEIPTS + 5 {
        state.select_clip(clip(), None);
        state.stop();
    }
    let runtime = ExtensionRuntime::default();
    let mut journal = LifecycleJournal::default();
    runtime.collect_motion_observations(&source);
    runtime.drain_observations(&mut journal);
    let events = journal.drain();
    assert!(
        matches!(events.last().unwrap().event,LifecycleEvent::Gap { dropped_records, dropped_receipts } if dropped_records == (2 * MAX_MOTION_RECEIPTS - 32) as u64 && dropped_receipts == 5)
    );
    runtime.collect_motion_observations(&source);
    runtime.drain_observations(&mut journal);
    assert!(journal.drain().is_empty());
}
#[test]
fn generic_extension_snapshot_is_immutable_and_survives_owned_receipt_discard() {
    let mut app = delivery_app();
    let source = app
        .world()
        .resource::<AnimationState>()
        .motion_observations();
    let target = crate::extension_protocol::Target {
        uuid: uuid::Uuid::new_v4(),
        generation: 9,
    };
    let snapshot = MotionSourceSnapshot {
        source_target: Some(target),
        motion_id: Some("org.example.pack/nod".into()),
        source: MotionPlaybackSource::Extension {
            integration_id: "org.example.test".into(),
            session_id: uuid::Uuid::new_v4(),
            process_generation: 2,
            action_id: "gesture-1".into(),
        },
    };
    {
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        let _tx = deferred_attributed(&mut state, true, false, snapshot.clone());
        state.import_owner = Some(PlaybackToken::new());
        state.import_receipt = Some(PlaybackReceipt::new());
        state.cancel_owned_requests();
    }
    let events = terminals(&source);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0],LifecycleEvent::MotionPlaybackEnded {source_target:Some(actual),motion_id,source:actual_source,outcome:MotionOutcome::Cancelled,..} if *actual==target && motion_id==&snapshot.motion_id && actual_source==&snapshot.source)
    );
}

#[test]
fn generic_import_keeps_admitted_identity_when_same_visible_file_is_replayed() {
    let mut app = delivery_app();
    let source = app
        .world()
        .resource::<AnimationState>()
        .motion_observations();
    let (tx, import_id, replay_id) = {
        let mut state = app.world_mut().resource_mut::<AnimationState>();
        state.select_clip(clip(), Some("local.bvh".into()));
        let tx = deferred(&mut state, false, false);
        let import_id = state.motion_import.as_ref().unwrap().id_for_test();
        state.play();
        let replay_id = state.motion_playback.as_ref().unwrap().id_for_test();
        (tx, import_id, replay_id)
    };
    let before = terminals(&source);
    assert_eq!(before.len(), 1);
    tx.send(Ok(("local.bvh".into(), parse_bvh(SAMPLE).unwrap())))
        .unwrap();
    app.update();
    let switched = terminals(&source);
    assert_eq!(switched.len(), 1);
    assert_eq!(
        terminal(&switched[0]),
        (replay_id, MotionOutcome::Cancelled)
    );
    app.world_mut().resource_mut::<AnimationState>().stop();
    let ended = terminals(&source);
    assert_eq!(ended.len(), 1);
    assert_eq!(terminal(&ended[0]), (import_id, MotionOutcome::Cancelled));
}

#[test]
fn activation_emits_once_with_admitted_identity_before_terminal_even_when_drained_together() {
    for (one_shot, blend) in [(false, false), (false, true), (true, false)] {
        let mut app = delivery_app();
        let source = app
            .world()
            .resource::<AnimationState>()
            .motion_observations();
        let tx = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            deferred(&mut state, one_shot, blend)
        };
        let identity = app
            .world()
            .resource::<AnimationState>()
            .motion_import
            .as_ref()
            .unwrap()
            .id_for_test();
        assert!(
            source.take_events().0.is_empty(),
            "admission is not activation"
        );
        tx.send(Ok(("local.bvh".into(), parse_bvh(SAMPLE).unwrap())))
            .unwrap();
        app.update();
        app.world_mut().resource_mut::<AnimationState>().stop();
        let (events, lost) = source.take_events();
        assert_eq!(lost, 0);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0].0, LifecycleEvent::MotionPlaybackStarted { playback_id, .. } if *playback_id==identity)
        );
        assert!(
            matches!(&events[1].0, LifecycleEvent::MotionPlaybackEnded { playback_id, outcome: MotionOutcome::Cancelled, .. } if *playback_id==identity)
        );
        assert!(events[0].1 <= events[1].1);
        app.update();
        assert!(source.take_events().0.is_empty());
    }
}
#[test]
fn failed_or_cancelled_import_never_invents_activation() {
    for cancel in [false, true] {
        let mut app = delivery_app();
        let source = app
            .world()
            .resource::<AnimationState>()
            .motion_observations();
        let tx = {
            let mut state = app.world_mut().resource_mut::<AnimationState>();
            deferred(&mut state, false, false)
        };
        if cancel {
            app.world_mut().resource_mut::<AnimationState>().stop();
        }
        tx.send(Err("private path is not event metadata".into()))
            .unwrap();
        app.update();
        let (events, lost) = source.take_events();
        assert_eq!(lost, 0);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].0,
            LifecycleEvent::MotionPlaybackEnded { .. }
        ));
    }
}
#[test]
fn activation_is_not_replayed_and_keeps_original_source_across_replacement() {
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    state.select_clip(clip(), None);
    let start = source.take_events().0;
    assert_eq!(start.len(), 1);
    let LifecycleEvent::MotionPlaybackStarted { playback_id, .. } = start[0].0 else {
        panic!("missing activation")
    };
    assert!(source.take_events().0.is_empty());
    state.pause();
    drop(state);
    let events = source.take_events().0;
    assert_eq!(events.len(), 1);
    assert_eq!(
        terminal(&events[0].0),
        (playback_id, MotionOutcome::Cancelled)
    );
}

#[test]
fn replay_and_park_restore_activate_fresh_requests_without_replaying_old_source() {
    let mut state = AnimationState::default();
    let source = state.motion_observations();
    state.select_clip(clip(), None);
    source.take_events();
    state.play();
    let pair = source.take_events().0;
    assert_eq!(pair.len(), 2);
    assert!(matches!(
        pair[0].0,
        LifecycleEvent::MotionPlaybackEnded { .. }
    ));
    assert!(matches!(
        pair[1].0,
        LifecycleEvent::MotionPlaybackStarted { .. }
    ));
    let previous = state.park_for_desktop();
    source.take_events();
    state.restore_parked(previous);
    let events = source.take_events().0;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].0,
        LifecycleEvent::MotionPlaybackStarted { .. }
    ));
}
