//! A composed frame whose fence cannot be passed to KMS stays pending. Readiness queries never wait;
//! the host retries only while a frame is outstanding, at a fraction of its refresh interval.
use smithay::backend::renderer::sync::SyncPoint;
use std::os::fd::OwnedFd;

#[derive(Default)]
pub(crate) struct DrawFence {
    pending: Option<SyncPoint>,
}
impl DrawFence {
    pub fn prepare(&mut self, sync: SyncPoint, native: bool) -> Option<OwnedFd> {
        let fence = native.then(|| sync.export()).flatten();
        self.pending = fence.is_none().then_some(sync);
        fence
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn ready(&self) -> bool {
        self.pending.as_ref().is_none_or(SyncPoint::is_reached)
    }
    pub fn clear(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::renderer::sync::{Fence, Interrupted};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    #[derive(Debug)]
    struct Unexportable(Arc<AtomicBool>, Arc<AtomicUsize>);
    impl Fence for Unexportable {
        fn is_signaled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
        fn wait(&self) -> Result<(), Interrupted> {
            self.1.fetch_add(1, Ordering::Relaxed);
            Err(Interrupted)
        }
        fn is_exportable(&self) -> bool {
            false
        }
        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }
    #[derive(Debug)]
    struct Exportable(OwnedFd);
    impl Fence for Exportable {
        fn is_signaled(&self) -> bool {
            false
        }
        fn wait(&self) -> Result<(), Interrupted> {
            Err(Interrupted)
        }
        fn is_exportable(&self) -> bool {
            true
        }
        fn export(&self) -> Option<OwnedFd> {
            self.0.try_clone().ok()
        }
    }
    #[test]
    #[allow(clippy::unwrap_used)]
    fn native_fences_are_handed_to_kms_without_waiting_for_readiness() {
        let (fd, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut fence = DrawFence::default();
        assert!(fence.prepare(Exportable(fd.into()).into(), true).is_some());
        assert!(!fence.pending());
        assert!(fence.ready(), "KMS owns the readiness dependency");
    }
    #[test]
    fn an_unfinished_frame_never_waits_or_prevents_another_screen_progressing() {
        let reached = Arc::new(AtomicBool::new(false));
        let waits = Arc::new(AtomicUsize::new(0));
        let mut first = DrawFence::default();
        assert!(first.prepare(Unexportable(reached.clone(), waits.clone()).into(), true).is_none(), "failed native exports take the same readiness path");
        for _ in 0..100 {
            assert!(!first.ready(), "wait() is not a readiness operation");
        }
        assert!(first.pending());
        assert_eq!(waits.load(Ordering::Relaxed), 0, "a pending fence never calls wait");
        let mut other = DrawFence::default();
        assert!(other.prepare(SyncPoint::signaled(), false).is_none());
        assert!(other.ready(), "another screen is independent");
        reached.store(true, Ordering::Release);
        assert!(first.ready());
        first.clear();
        assert!(!first.pending());
        assert!(first.ready());
    }
    #[test]
    fn abandoning_a_frame_clears_its_wait_without_querying_or_waiting() {
        let mut fence = DrawFence::default();
        let _ = fence.prepare(Unexportable(Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0))).into(), false);
        fence.clear();
        assert!(!fence.pending());
        assert!(fence.ready());
    }
}
