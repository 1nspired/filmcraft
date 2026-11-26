//! Effect-free composition only. Full caller fences bracket every observation,
//! including failed reads. Longer reads revisit the same source within a bounded
//! number of chunks or elapsed time; no filesystem observation is cached and no
//! writer may use this callback in place of its original effect guard.
use std::{
    cell::Cell,
    io,
    time::{Duration, Instant},
};

const MAX_OBSERVATIONS: u32 = 64;
const SOURCE_INTERVAL: Duration = Duration::from_millis(250);

pub(super) struct ReadPass<'a, F> {
    source: &'a F,
    observations: Cell<u32>,
    last_source: Cell<Instant>,
}
impl<F: Fn() -> io::Result<()>> ReadPass<'_, F> {
    pub(super) fn check(&self) -> io::Result<()> {
        let count = self.observations.get().saturating_add(1);
        if count >= MAX_OBSERVATIONS || self.last_source.get().elapsed() >= SOURCE_INTERVAL {
            (self.source)()?;
            self.observations.set(0);
            self.last_source.set(Instant::now());
        } else {
            self.observations.set(count);
        }
        Ok(())
    }
}

pub(super) fn observe<T, F: Fn() -> io::Result<()>>(source: &F, read: impl FnOnce(&ReadPass<'_, F>) -> io::Result<T>) -> io::Result<T> {
    observe_with_fence(source, source, read)
}

pub(super) fn observe_with_fence<T, F: Fn() -> io::Result<()>>(
    source: &F,
    after_read: &impl Fn() -> io::Result<()>,
    read: impl FnOnce(&ReadPass<'_, F>) -> io::Result<T>,
) -> io::Result<T> {
    source()?;
    let pass = ReadPass { source, observations: Cell::new(0), last_source: Cell::new(Instant::now()) };
    let observed = read(&pass);
    // Always finish the SAME complete read, even on observation failure. The
    // default API repeats the complete source; typed callers may supply the
    // original generation/Stop fence. This never crosses a writer boundary.
    after_read()?;
    observed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refused_initial_source_does_not_enter_read() {
        let entered = Cell::new(false);
        let result = observe(&|| Err(io::Error::other("source lost")), |_| {
            entered.set(true);
            Ok(())
        });
        assert!(result.is_err());
        assert!(!entered.get());
    }
    #[test]
    fn failed_read_still_checks_original_source_and_returns_its_refusal() {
        let calls = Cell::new(0);
        let source = || {
            calls.set(calls.get() + 1);
            if calls.get() == 2 { Err(io::Error::other("source revoked")) } else { Ok(()) }
        };
        let result: io::Result<()> = observe(&source, |_| Err(io::Error::other("read failed")));
        assert_eq!(calls.get(), 2);
        assert_eq!(result.unwrap_err().to_string(), "source revoked");
    }
    #[test]
    fn source_stop_during_read_refuses_the_observation() {
        let live = Cell::new(true);
        let result = observe(&|| if live.get() { Ok(()) } else { Err(io::Error::other("Stop")) }, |pass| {
            pass.check()?;
            live.set(false);
            Ok(17)
        });
        assert_eq!(result.unwrap_err().to_string(), "Stop");
    }
    #[test]
    fn long_chunked_reads_revisit_same_source_before_read_can_finish() {
        let calls = Cell::new(0);
        let source = || {
            calls.set(calls.get() + 1);
            if calls.get() >= 2 { Err(io::Error::other("source lost during read")) } else { Ok(()) }
        };
        let result = observe(&source, |pass| {
            for _ in 0..MAX_OBSERVATIONS {
                pass.check()?;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(calls.get() >= 3);
    }
    #[test]
    fn elapsed_read_revisits_source_even_with_fewer_chunks() {
        let calls = Cell::new(0);
        let source = || {
            calls.set(calls.get() + 1);
            Ok(())
        };
        observe(&source, |pass| {
            pass.last_source.set(Instant::now().checked_sub(SOURCE_INTERVAL).unwrap());
            pass.check()?;
            assert_eq!(calls.get(), 2);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 3);
    }
    #[test]
    fn paired_final_fence_runs_on_failed_observation_without_replacing_initial_complete_source() {
        let full = Cell::new(0);
        let fenced = Cell::new(0);
        let current = || {
            full.set(full.get() + 1);
            Ok(())
        };
        let after = || {
            fenced.set(fenced.get() + 1);
            Err(io::Error::other("original final fence refused"))
        };
        let result: io::Result<()> = observe_with_fence(&current, &after, |_| Err(io::Error::other("read failed")));
        assert_eq!(result.unwrap_err().to_string(), "original final fence refused");
        assert_eq!(full.get(), 1);
        assert_eq!(fenced.get(), 1);
    }
    #[test]
    fn paired_long_read_revisits_complete_source_before_its_final_fence() {
        let full = Cell::new(0);
        let fenced = Cell::new(0);
        let current = || {
            full.set(full.get() + 1);
            Ok(())
        };
        let after = || {
            fenced.set(fenced.get() + 1);
            Ok(())
        };
        observe_with_fence(&current, &after, |pass| {
            for _ in 0..MAX_OBSERVATIONS {
                pass.check()?;
            }
            assert_eq!(full.get(), 2);
            Ok(())
        })
        .unwrap();
        assert_eq!(full.get(), 2);
        assert_eq!(fenced.get(), 1);
    }
    #[test]
    fn paired_separate_read_scopes_always_repeat_complete_source_and_final_fence() {
        let full = Cell::new(0);
        let fenced = Cell::new(0);
        let current = || {
            full.set(full.get() + 1);
            Ok(())
        };
        let after = || {
            fenced.set(fenced.get() + 1);
            Ok(())
        };
        for _ in 0..2 {
            observe_with_fence(&current, &after, |pass| pass.check()).unwrap();
        }
        assert_eq!(full.get(), 2);
        assert_eq!(fenced.get(), 2);
    }
}
