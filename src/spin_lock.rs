//! Definition and implementations of `SpinLock`
//!
use util;
use std::intrinsics;
use std::ptr;

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
    pub fn lock(&mut self) {
        while self.is_locked() || !unsafe { self.inner_lock() } {
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
    pub fn unlock(&mut self) {
        assert!(self.is_locked() && unsafe { self.inner_unlock() });
    }

    #[inline]
    unsafe fn inner_unlock(&mut self) -> bool {
        intrinsics::atomic_cxchg(&mut self.atomic, 1, 0).1
    }

    #[inline]
    unsafe fn inner_lock(&mut self) -> bool {
        intrinsics::atomic_cxchg(&mut self.atomic, 0, 1).1
    }

    /// Return true if locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        unsafe { 0 != intrinsics::atomic_load(&self.atomic) }
    }

    /// Return true if lock successfully.
    #[inline]
    pub fn try_lock(&mut self) -> bool {
        !self.is_locked() && unsafe { self.inner_lock() }
    }
}

/// Guard of SpinLock, unlock it when dropped.
pub struct SpinLockGuard {
    spin_lock: *mut SpinLock,
}

impl Default for SpinLockGuard {
    fn default() -> Self {
        SpinLockGuard {
            spin_lock: ptr::null_mut(),
        }
    }
}

impl SpinLockGuard {
    #[inline]
    unsafe fn destroy(&mut self) {
        if !self.spin_lock.is_null() {
            (*self.spin_lock).unlock();
            self.spin_lock = ptr::null_mut();
        }
    }

    #[inline]
    fn new(spin_lock: *mut SpinLock) -> Self {
        SpinLockGuard { spin_lock }
    }
}

impl Drop for SpinLockGuard {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}

mod test {
    #[test]
    fn test_spin_lock() {
        use spin_lock::SpinLock;
        let mut lock = SpinLock::default();
        lock.lock();
        assert!(lock.is_locked());
        lock.unlock();
        assert!(!lock.is_locked());

        unsafe {
            let _lock_guard = lock.lock_guard();
            assert!(lock.is_locked());
        }
        assert!(!lock.is_locked());
    }
}
