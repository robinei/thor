//! CPU submission and asynchronous GPU readiness, observed without waiting or polling.
use crate::server::Server;
use smithay::{
    backend::renderer::sync::SyncPoint,
    reexports::calloop::{Dispatcher, Interest, LoopHandle, Mode, PostAction, RegistrationToken, generic::Generic, transient::TransientSource},
};
use std::{
    cell::RefCell,
    os::fd::OwnedFd,
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    pub cpu: Duration,
    /// Drawing start to fence observation, including queueing and event-loop wakeup latency.
    pub ready: Duration,
}
#[derive(Default)]
struct Observations {
    started: Option<Instant>,
    cpu: Duration,
    sample: Option<Sample>,
}
impl Observations {
    fn complete(&mut self, now: Instant) {
        if let Some(started) = self.started.take() {
            self.sample = Some(Sample { cpu: self.cpu, ready: now.saturating_duration_since(started).max(self.cpu) });
        }
    }
}

type Source = TransientSource<Generic<OwnedFd>>;
struct Watch {
    source: Dispatcher<'static, Source, Server>,
    token: RegistrationToken,
    handle: LoopHandle<'static, Server>,
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.handle.remove(self.token);
    }
}

/// One reusable fence source per output. It has no registered fd while idle.
#[derive(Default)]
pub(crate) struct Monitor {
    observations: Rc<RefCell<Observations>>,
    watch: Option<Watch>,
    failed: bool,
}
impl Monitor {
    pub fn begin(&mut self, now: Instant) {
        self.cancel();
        self.observations.borrow_mut().started = Some(now);
    }
    pub fn cancel(&mut self) {
        self.observations.borrow_mut().started = None;
        self.observations.borrow_mut().sample = None;
        if let Some(watch) = &mut self.watch {
            watch.source.as_source_mut().remove();
            if let Err(error) = watch.handle.update(&watch.token) {
                log::warn!("frame timing: cancelling fence: {error}");
            }
        }
    }
    pub fn sample(&mut self) -> Option<Sample> {
        self.observations.borrow_mut().sample.take()
    }

    pub fn submitted(&mut self, sync: &SyncPoint, handle: &LoopHandle<'static, Server>) {
        let now = Instant::now();
        {
            let mut observations = self.observations.borrow_mut();
            let Some(started) = observations.started else { return };
            observations.cpu = now.saturating_duration_since(started);
            if sync.is_reached() {
                observations.complete(now);
                return;
            }
        }
        if self.failed {
            return;
        }
        let Some(fd) = sync.export() else {
            self.failed = true;
            log::warn!("frame timing: GPU fence cannot be exported; adaptive scheduling will use a full refresh budget");
            return;
        };
        let generic = Generic::new(fd, Interest::READ, Mode::Level);
        if self.watch.is_none() {
            let observations = self.observations.clone();
            let source = Dispatcher::new(Source::from(generic), move |_, _, _: &mut Server| {
                observations.borrow_mut().complete(Instant::now());
                Ok(PostAction::Remove)
            });
            match handle.register_dispatcher(source.clone()) {
                Ok(token) => self.watch = Some(Watch { source, token, handle: handle.clone() }),
                Err(error) => {
                    self.failed = true;
                    log::warn!("frame timing: GPU fence source unavailable: {error}");
                    return;
                }
            }
            return;
        }
        if let Some(watch) = &mut self.watch {
            {
                let mut source = watch.source.as_source_mut();
                if source.is_none() {
                    *source = Source::from(generic);
                } else {
                    source.replace(generic);
                }
            }
            if let Err(error) = watch.handle.update(&watch.token) {
                self.failed = true;
                log::warn!("frame timing: registering GPU fence: {error}");
                self.cancel();
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn completion_includes_queue_and_wakeup_delay_and_is_consumed_once() {
        let start = Instant::now();
        let mut monitor = Monitor::default();
        monitor.begin(start);
        monitor.observations.borrow_mut().cpu = Duration::from_micros(300);
        monitor.observations.borrow_mut().complete(start + Duration::from_millis(2));
        let sample = monitor.sample().unwrap();
        assert_eq!(sample.cpu, Duration::from_micros(300));
        assert_eq!(sample.ready, Duration::from_millis(2));
        assert!(monitor.sample().is_none());
        monitor.begin(start);
        monitor.cancel();
        monitor.observations.borrow_mut().complete(start + Duration::from_secs(1));
        assert!(monitor.sample().is_none(), "a cancelled drawing never becomes a sample");
    }
    #[test]
    fn a_reusable_fence_source_wakes_once_per_drawing_and_never_waits() {
        use smithay::backend::renderer::sync::{Fence, Interrupted};
        use std::{
            io::Write,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
        };
        #[derive(Debug)]
        struct FakeFence(OwnedFd, Arc<AtomicUsize>);
        impl Fence for FakeFence {
            fn is_signaled(&self) -> bool {
                false
            }
            fn wait(&self) -> Result<(), Interrupted> {
                self.1.fetch_add(1, Ordering::Relaxed);
                Err(Interrupted)
            }
            fn is_exportable(&self) -> bool {
                true
            }
            fn export(&self) -> Option<OwnedFd> {
                self.0.try_clone().ok()
            }
        }
        let mut event_loop = smithay::reexports::calloop::EventLoop::try_new().unwrap();
        let (_display, mut server) = crate::tests::ServerBuilder::new().started();
        let waits = Arc::new(AtomicUsize::new(0));
        let mut monitor = Monitor::default();
        for _ in 0..3 {
            let (read, mut write) = std::os::unix::net::UnixStream::pair().unwrap();
            let sync: SyncPoint = FakeFence(read.into(), waits.clone()).into();
            monitor.begin(Instant::now());
            monitor.submitted(&sync, &event_loop.handle());
            assert!(monitor.sample().is_none());
            write.write_all(b"x").unwrap();
            event_loop.dispatch(Some(Duration::from_secs(1)), &mut server).unwrap();
            let sample = monitor.sample().unwrap();
            assert!(sample.ready >= sample.cpu);
            assert!(monitor.sample().is_none());
            event_loop.dispatch(Some(Duration::ZERO), &mut server).unwrap();
            assert!(monitor.sample().is_none(), "a readable old fence must be unregistered");
        }
        assert_eq!(waits.load(Ordering::Relaxed), 0);
    }
}
