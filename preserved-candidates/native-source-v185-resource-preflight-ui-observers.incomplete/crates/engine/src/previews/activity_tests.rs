use super::*;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::channel,
    },
    time::{Duration, Instant},
};
fn paused(activity: &Activity) {
    let began = Instant::now();
    while !activity.state.lock().unwrap().paused {
        assert!(began.elapsed() < Duration::from_secs(2));
        std::thread::yield_now();
    }
}
#[test]
fn preview_save_pause_waits_for_the_last_original_writer_and_refuses_new_or_duplicate_owners() {
    let activity = Arc::new(Activity::default());
    let writer = activity.writer().unwrap();
    let (tx, rx) = channel();
    let copy = activity.clone();
    let worker = std::thread::spawn(move || {
        let held = copy.pause(&|| Ok(())).unwrap();
        tx.send(held).unwrap();
    });
    paused(&activity);
    assert!(activity.writer().is_err());
    assert!(activity.pause(&|| Ok(())).is_err());
    assert!(matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    drop(writer);
    let held = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
    assert!(activity.writer().is_err());
    drop(held);
    drop(activity.writer().unwrap());
}
#[test]
fn preview_save_pause_stop_or_deadline_releases_the_gate_without_cancelling_a_human_writer() {
    let activity = Arc::new(Activity::default());
    let writer = activity.writer().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let copy = activity.clone();
    let signal = stop.clone();
    let worker =
        std::thread::spawn(move || copy.pause(&|| if signal.load(Ordering::Acquire) { Err(error("generated Stop/deadline")) } else { Ok(()) }).is_err());
    paused(&activity);
    stop.store(true, Ordering::Release);
    assert!(worker.join().unwrap());
    let state = activity.state.lock().unwrap();
    assert_eq!(state.writers, 1);
    assert!(!state.paused);
    drop(state);
    drop(writer);
    drop(activity.pause(&|| Ok(())).unwrap());
}
#[test]
fn original_preview_job_panic_guard_and_unlaunched_closure_drop_both_release_the_actual_writer_lease() {
    let activity = Arc::new(Activity::default());
    let writing = activity.writer().unwrap();
    let progress = Arc::default();
    let result = Arc::new(Mutex::new(None));
    let run = crate::export_tools::guard_job(progress, result.clone(), move || {
        let _writing = writing;
        panic!("generated original preview worker failure");
    });
    std::thread::spawn(run).join().unwrap();
    assert!(result.lock().unwrap().as_ref().unwrap().is_err());
    drop(activity.pause(&|| Ok(())).unwrap());
    let writing = activity.writer().unwrap();
    let unlaunched = move || {
        let _writing = writing;
    };
    drop(unlaunched);
    drop(activity.pause(&|| Ok(())).unwrap());
}
#[test]
fn preview_writer_exhaustion_and_cancelled_pause_refuse_without_poisoning_or_stranding_the_store() {
    let activity = Arc::new(Activity::default());
    activity.state.lock().unwrap().writers = usize::MAX;
    assert!(activity.writer().is_err());
    activity.state.lock().unwrap().writers = 0;
    assert!(activity.pause(&|| Err(error("stopped before queue"))).is_err());
    drop(activity.writer().unwrap());
    drop(activity.pause(&|| Ok(())).unwrap());
}

#[test]
fn preview_observer_checks_exact_pause_epoch_and_cannot_accept_a_replacement_pause() {
    let a = Arc::new(Activity::default());
    let held = a.pause(&|| Ok(())).unwrap();
    let probe = held.probe();
    probe.current().unwrap();
    assert!(a.writer().is_err());
    drop(held);
    assert!(probe.current().is_err());
    let next = a.pause(&|| Ok(())).unwrap();
    next.probe().current().unwrap();
    assert!(probe.current().is_err());
    drop(next);
    drop(a.writer().unwrap());
}
