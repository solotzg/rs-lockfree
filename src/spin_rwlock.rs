use std::ptr;
use std::intrinsics;
use util;

const MAX_REF_CNT: u64 = 0x00ffffff;

#[repr(C)]
#[derive(Copy, Clone)]
union AtomicLockData {
    v: u64,
    rw_info: u64,
}

#[derive(Copy, Clone)]
struct Atomic {
    data: AtomicLockData,
}

impl Atomic {
    #[inline]
    unsafe fn v(&self) -> u64 {
        self.data.v
    }

    #[inline]
    unsafe fn v_mut(&mut self) -> &mut u64 {
        &mut self.data.v
    }

    #[inline]
    unsafe fn r_ref_cnt(&self) -> u64 {
        // 62b
        self.data.rw_info & 0x3fffffffffffffff
    }

    #[inline]
    unsafe fn w_pending(&self) -> u64 {
        // 1b
        (self.data.rw_info & 0x4000000000000000) >> 62
    }

    #[inline]
    unsafe fn w_lock_flag(&self) -> u64 {
        // 1b
        (self.data.rw_info & 0x8000000000000000) >> 63
    }

    #[inline]
    unsafe fn set_r_ref_cnt(&mut self, r_ref_cnt: u64) {
        self.data.rw_info |= r_ref_cnt & 0x3fffffffffffffff;
    }

    #[inline]
    unsafe fn add_r_ref_cnt(&mut self, cnt: u64) {
        let cnt = self.r_ref_cnt() + cnt;
        self.set_r_ref_cnt(cnt);
    }

    #[inline]
    unsafe fn sub_r_ref_cnt(&mut self, cnt: u64) {
        let cnt = self.r_ref_cnt() - cnt;
        self.set_r_ref_cnt(cnt);
    }

    #[inline]
    unsafe fn set_w_pending(&mut self, w_pending: u64) {
        self.data.rw_info |= (w_pending & 0x1) << 62;
    }

    #[inline]
    unsafe fn set_w_lock_flag(&mut self, w_lock_flag: u64) {
        self.data.rw_info |= (w_lock_flag & 0x1) << 63;
    }

    pub fn new_from_other(other: &Atomic) -> Atomic {
        unsafe { ptr::read_volatile(other) }
    }
}

impl Default for Atomic {
    fn default() -> Self {
        Atomic {
            data: AtomicLockData { v: 0 },
        }
    }
}

struct SpinRWLock {
    atomic: Atomic,
    w_owner: i64,
}

impl SpinRWLock {
    #[inline]
    unsafe fn atomic(&self) -> Atomic {
        ptr::read_volatile(&self.atomic)
    }

    unsafe fn try_rlock(&mut self) -> bool {
        let mut ret = false;
        let old_v = self.atomic();
        let mut new_v = old_v;
        new_v.add_r_ref_cnt(1);
        if 0 == old_v.w_pending() && 0 == old_v.w_lock_flag() && MAX_REF_CNT > old_v.r_ref_cnt()
            && intrinsics::atomic_cxchg(self.atomic.v_mut(), old_v.v(), new_v.v()).1
        {
            ret = true;
        }
        ret
    }

    unsafe fn rlock(&mut self) {
        loop {
            let old_v = self.atomic();
            let mut new_v = old_v;
            new_v.add_r_ref_cnt(1);
            if 0 == old_v.w_pending() && 0 == old_v.w_lock_flag() && MAX_REF_CNT > old_v.r_ref_cnt()
                && intrinsics::atomic_cxchg(self.atomic.v_mut(), old_v.v(), new_v.v()).1
            {
                break;
            }
            util::pause();
        }
    }

    unsafe fn unrlock(&mut self) {
        loop {
            let old_v = self.atomic();
            let mut new_v = old_v;
            new_v.sub_r_ref_cnt(1);
            if 0 != old_v.w_lock_flag() || 0 == old_v.r_ref_cnt() || MAX_REF_CNT < old_v.r_ref_cnt()
            {
                panic!("this should never happen");
            } else if intrinsics::atomic_cxchg(self.atomic.v_mut(), old_v.v(), new_v.v()).1 {
                break;
            } else {
                util::pause();
            }
        }
    }

    unsafe fn try_lock(&mut self) -> bool {
        let mut ret = false;
        let old_v = self.atomic();
        let mut new_v = old_v;
        new_v.set_w_pending(0);
        new_v.set_w_lock_flag(1);
        if 0 == old_v.w_pending() && 0 == old_v.w_lock_flag() && MAX_REF_CNT > old_v.r_ref_cnt()
            && intrinsics::atomic_cxchg(self.atomic.v_mut(), old_v.v(), new_v.v()).1
        {
            ret = true;
        }
        ret
    }

    unsafe fn lock(&mut self) {
        loop {
            let old_v = self.atomic();
            let mut new_v = old_v;
            let mut pending = false;
            if 0 != old_v.w_lock_flag() || 0 != old_v.r_ref_cnt() {
                new_v.set_w_pending(1);
                pending = true;
            } else {
                new_v.set_w_pending(0);
                new_v.set_w_lock_flag(1);
            }
            if intrinsics::atomic_cxchg(self.atomic.v_mut(), old_v.v(), new_v.v()).1 {
                if !pending {
                    self.w_owner = util::get_thread_id();
                    break;
                }
            }
            util::pause();
        }
    }

    unsafe fn unlock(&mut self) {
        loop {
            let old_v = self.atomic();
            let mut new_v = old_v;
            new_v.set_w_lock_flag(0);
            if 0 == old_v.w_lock_flag() || 0 != old_v.r_ref_cnt() {
                panic!("this should never happen");
            } else if intrinsics::atomic_cxchg(self.atomic.v_mut(), old_v.v(), new_v.v()).1 {
                break;
            } else {
                util::pause();
            }
        }
    }
}

impl Default for SpinRWLock {
    fn default() -> Self {
        SpinRWLock {
            atomic: Default::default(),
            w_owner: 0,
        }
    }
}

struct RLockGuard {
    lock: *mut SpinRWLock,
}

impl RLockGuard {
    unsafe fn destroy(&mut self) {
        if !self.lock.is_null() {
            (*self.lock).unlock();
            self.lock = ptr::null_mut();
        }
    }

    pub fn set_lock(&mut self, lock: *mut SpinRWLock) {
        self.lock = lock;
    }
}

impl Default for RLockGuard {
    fn default() -> Self {
        RLockGuard {
            lock: ptr::null_mut(),
        }
    }
}

struct WLockGuard {
    lock: *mut SpinRWLock,
}

impl WLockGuard {
    unsafe fn destroy(&mut self) {
        if !self.lock.is_null() {
            (*self.lock).unlock();
            self.lock = ptr::null_mut();
        }
    }

    pub fn set_lock(&mut self, lock: *mut SpinRWLock) {
        self.lock = lock;
    }
}

impl Default for WLockGuard {
    fn default() -> Self {
        WLockGuard {
            lock: ptr::null_mut(),
        }
    }
}
