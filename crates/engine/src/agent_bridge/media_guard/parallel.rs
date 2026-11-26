//! One optional extra observer slot per engine process. A busy slot falls back
//! to the identical sequential proof; no queue, retry, prefix or cached hash.
use crate::Result;
use std::sync::atomic::{AtomicBool, Ordering};

static EXTRA: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
pub(super) static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Extra;
impl Extra {
    fn acquire() -> Option<Self> {
        EXTRA.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok().map(|_| Self)
    }
}
impl Drop for Extra {
    fn drop(&mut self) {
        EXTRA.store(false, Ordering::Release);
    }
}

fn lane<T>(count: usize, start: usize, step: usize, work: &(impl Fn(usize) -> Result<T> + Sync)) -> Result<Vec<(usize, T)>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (start..count).step_by(step).map(|index| work(index).map(|value| (index, value))).collect()))
        .unwrap_or_else(|_| Err(super::error("original-file observer failed; no file authority was issued")))
}

pub(super) fn sweep<T: Send>(count: usize, parallel: bool, work: impl Fn(usize) -> Result<T> + Sync) -> Result<Vec<T>> {
    if count > 2048 {
        return Err(super::error("original observer sweep exceeds its bounded input count"));
    }
    let extra = (parallel && count > 1).then(Extra::acquire).flatten();
    let mut rows = if let Some(_extra) = extra {
        // Hold the permit until the scoped child is joined, including failure.
        std::thread::scope(|scope| -> Result<Vec<(usize, T)>> {
            let work = &work;
            let worker = std::thread::Builder::new()
                .name("poema-original-observer".into())
                .spawn_scoped(scope, move || lane(count, 1, 2, work))
                .map_err(|_| super::error("original-file observer could not start"))?;
            let first = lane(count, 0, 2, work);
            let second = worker.join().map_err(|_| super::error("original-file observer did not finish safely"));
            // Join before propagating either failure: cancellation cannot free
            // a reader slot while its thread still owns a file observation.
            let mut rows = first?;
            rows.extend(second??);
            Ok(rows)
        })?
    } else {
        lane(count, 0, 1, &work)?
    };
    rows.sort_by_key(|(index, _)| *index);
    Ok(rows.into_iter().map(|(_, value)| value).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Mutex, atomic::AtomicUsize};

    #[test]
    fn two_observer_lanes_preserve_order_and_a_busy_slot_uses_the_same_sequential_work() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let barrier = Barrier::new(2);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let rows = sweep(6, true, |i| {
            let n = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            if i < 2 {
                barrier.wait();
            }
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(i)
        })
        .unwrap();
        assert_eq!(rows, (0..6).collect::<Vec<_>>());
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert!(!EXTRA.load(Ordering::Acquire));
        let held = Extra::acquire().unwrap();
        let seen = Mutex::new(Vec::new());
        assert_eq!(
            sweep(6, true, |i| {
                seen.lock().unwrap().push(i);
                Ok(i)
            })
            .unwrap(),
            rows
        );
        assert_eq!(*seen.lock().unwrap(), rows);
        drop(held);
        assert!(!EXTRA.load(Ordering::Acquire));
    }

    #[test]
    fn failed_or_panicking_observers_join_and_release_the_extra_slot_without_authority() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for failing in [0, 1] {
            assert!(sweep(2, true, |i| { if i == failing { Err(super::super::error("generated observer error")) } else { Ok(i) } }).is_err());
            assert!(!EXTRA.load(Ordering::Acquire));
            let caught = std::panic::catch_unwind(|| {
                sweep(2, true, |i| {
                    assert_ne!(i, failing, "generated observer panic");
                    Ok(i)
                })
            });
            assert!(caught.is_ok());
            assert!(caught.unwrap().is_err());
            assert!(!EXTRA.load(Ordering::Acquire));
        }
        assert!(sweep(2049, true, Ok::<_, crate::EngineError>).is_err());
    }
}
