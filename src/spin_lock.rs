use util;
use std::intrinsics;

/// User mode SpinLock
pub struct SpinLock {
    atomic: i8,
}

impl Default for SpinLock {
    fn default() -> Self {
        SpinLock { atomic: 0 }
    }
}

impl SpinLock {
    /// Keep trying to lock until success.
    pub unsafe fn lock(&mut self) {
        while self.is_locked() || !intrinsics::atomic_cxchg(&mut self.atomic, 0, 1).1 {
            util::pause();
        }
    }

    /// Keep trying to lock until success, then return SpinLockGuard.
    #[inline]
    pub unsafe fn lock_guard(&mut self) -> SpinLockGuard {
        self.lock();
        SpinLockGuard::new(self)
    }

    /// Unlock if is locked, else panic.
    #[inline]
    pub unsafe fn unlock(&mut self) {
        assert!(self.is_locked() && intrinsics::atomic_cxchg(&mut self.atomic, 1, 0).1);
    }

    /// Return true if locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        unsafe { 0 != intrinsics::atomic_load(&self.atomic) }
    }

    /// Return true if lock successfully.
    #[inline]
    pub unsafe fn try_lock(&mut self) -> bool {
        !self.is_locked() && intrinsics::atomic_cxchg(&mut self.atomic, 0, 1).1
    }
}

/// Guard of SpinLock, unlock it when dropped.
pub struct SpinLockGuard<'a> {
    spin_lock: &'a mut SpinLock,
}

impl<'a> SpinLockGuard<'a> {
    unsafe fn destroy(&mut self) {
        self.spin_lock.unlock();
    }

    fn new(spin_lock: &'a mut SpinLock) -> Self {
        SpinLockGuard { spin_lock }
    }
}

impl<'a> Drop for SpinLockGuard<'a> {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}
