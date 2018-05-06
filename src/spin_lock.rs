use util;
use std::intrinsics;
use std::ptr;

pub struct SpinLock {
    atomic: i8,
}

impl Default for SpinLock {
    fn default() -> Self {
        SpinLock { atomic: 0 }
    }
}

impl SpinLock {
    pub unsafe fn lock(&mut self) {
        while !(0 == intrinsics::atomic_load(&self.atomic)
            && intrinsics::atomic_cxchg(&mut self.atomic, 0, 1).1)
        {
            util::pause();
        }
    }

    pub unsafe fn unlock(&mut self) {
        assert!(
            1 == intrinsics::atomic_load(&self.atomic)
                && intrinsics::atomic_cxchg(&mut self.atomic, 1, 0).1
        );
    }

    pub unsafe fn try_lock(&mut self) -> bool {
        (0 == intrinsics::atomic_load(&self.atomic)
            && intrinsics::atomic_cxchg(&mut self.atomic, 0, 1).1)
    }
}

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
    unsafe fn destroy(&mut self) {
        if !self.spin_lock.is_null() {
            (*self.spin_lock).unlock();
            self.spin_lock = ptr::null_mut();
        }
    }

    pub fn set_spin_lock(&mut self, spin_lock: *mut SpinLock) {
        self.spin_lock = spin_lock;
    }
}

impl Drop for SpinLockGuard {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}
