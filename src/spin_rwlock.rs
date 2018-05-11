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
struct AtomicInfo {
    data: AtomicLockData,
}

impl AtomicInfo {
    #[inline]
    pub fn v(&self) -> u64 {
        unsafe { self.data.v }
    }

    #[inline]
    pub fn v_mut(&mut self) -> &mut u64 {
        unsafe { &mut self.data.v }
    }

    #[inline]
    pub fn v_ref(&self) -> &u64 {
        unsafe { &self.data.v }
    }

    #[inline]
    pub fn r_ref_cnt(&self) -> u64 {
        // 62b
        unsafe { self.data.rw_info & 0x3fffffffffffffff }
    }

    #[inline]
    pub fn w_pending(&self) -> u64 {
        // 1b
        unsafe { (self.data.rw_info & 0x4000000000000000) >> 62 }
    }

    #[inline]
    pub fn w_lock_flag(&self) -> u64 {
        // 1b
        unsafe { (self.data.rw_info & 0x8000000000000000) >> 63 }
    }

    #[inline]
    pub fn set_r_ref_cnt(&mut self, r_ref_cnt: u64) {
        unsafe {
            self.data.rw_info =
                (self.data.rw_info & 0xc000000000000000) | (r_ref_cnt & 0x3fffffffffffffff);
        }
    }

    #[inline]
    pub fn add_r_ref_cnt(&mut self, cnt: u64) {
        let cnt = self.r_ref_cnt() + cnt;
        self.set_r_ref_cnt(cnt);
    }

    #[inline]
    pub fn sub_r_ref_cnt(&mut self, cnt: u64) {
        let cnt = self.r_ref_cnt() - cnt;
        self.set_r_ref_cnt(cnt);
    }

    #[inline]
    pub fn set_w_pending(&mut self, w_pending: u64) {
        unsafe {
            self.data.rw_info =
                (self.data.rw_info & 0xbfffffffffffffff) | ((w_pending & 0x1) << 62);
        }
    }

    #[inline]
    pub fn set_w_lock_flag(&mut self, w_lock_flag: u64) {
        unsafe {
            self.data.rw_info =
                (self.data.rw_info & 0x7fffffffffffffff) | ((w_lock_flag & 0x1) << 63);
        }
    }

    pub fn new(v: u64) -> Self {
        AtomicInfo {
            data: AtomicLockData { v },
        }
    }
}

impl Default for AtomicInfo {
    fn default() -> Self {
        AtomicInfo::new(0)
    }
}

pub struct SpinRWLock {
    atomic_info: AtomicInfo,
    w_owner: i64,
}

impl SpinRWLock {
    #[inline]
    fn atomic_info(&self) -> AtomicInfo {
        // TODO: whether atomic_load is needed or not?
        self.atomic_info
    }

    #[inline]
    fn atomic_cxchg_atomic_v(&mut self, old_v: u64, new_v: u64) -> bool {
        unsafe { intrinsics::atomic_cxchg(self.atomic_info.v_mut(), old_v, new_v).1 }
    }

    #[inline]
    pub fn try_rlock(&mut self) -> bool {
        let mut ret = false;
        let old_v = self.atomic_info();
        let mut new_v = old_v;
        new_v.add_r_ref_cnt(1);
        if 0 == old_v.w_pending() && 0 == old_v.w_lock_flag() && MAX_REF_CNT > old_v.r_ref_cnt()
            && self.atomic_cxchg_atomic_v(old_v.v(), new_v.v())
        {
            ret = true;
        }
        ret
    }

    pub fn rlock(&mut self) {
        loop {
            let old_v = self.atomic_info();
            let mut new_v = old_v;
            new_v.add_r_ref_cnt(1);
            if 0 == old_v.w_pending() && 0 == old_v.w_lock_flag() && MAX_REF_CNT > old_v.r_ref_cnt()
                && self.atomic_cxchg_atomic_v(old_v.v(), new_v.v())
            {
                break;
            }
            util::pause();
        }
    }

    pub unsafe fn unrlock(&mut self) {
        loop {
            let old_v = self.atomic_info();
            let mut new_v = old_v;
            new_v.sub_r_ref_cnt(1);
            if 0 != old_v.w_lock_flag() || 0 == old_v.r_ref_cnt() || MAX_REF_CNT < old_v.r_ref_cnt()
            {
                panic!("this should never happen");
            } else if self.atomic_cxchg_atomic_v(old_v.v(), new_v.v()) {
                break;
            } else {
                util::pause();
            }
        }
    }

    #[inline]
    pub fn try_lock(&mut self) -> bool {
        let mut ret = false;
        let old_v = self.atomic_info();
        let mut new_v = old_v;
        new_v.set_w_pending(0);
        new_v.set_w_lock_flag(1);
        if 0 == old_v.w_lock_flag() && 0 == old_v.r_ref_cnt()
            && self.atomic_cxchg_atomic_v(old_v.v(), new_v.v())
        {
            ret = true;
        }
        ret
    }

    pub fn lock(&mut self) {
        loop {
            let old_v = self.atomic_info();
            let mut new_v = old_v;
            let mut pending = false;
            if 0 != old_v.w_lock_flag() || 0 != old_v.r_ref_cnt() {
                new_v.set_w_pending(1);
                pending = true;
            } else {
                new_v.set_w_pending(0);
                new_v.set_w_lock_flag(1);
            }
            if self.atomic_cxchg_atomic_v(old_v.v(), new_v.v()) {
                if !pending {
                    self.w_owner = util::get_thread_id();
                    assert_eq!(new_v.w_pending(), 0);
                    break;
                }
            }
            util::pause();
        }
    }

    pub unsafe fn unlock(&mut self) {
        loop {
            let old_v = self.atomic_info();
            let mut new_v = old_v;
            new_v.set_w_lock_flag(0);
            if 0 == old_v.w_lock_flag() || 0 != old_v.r_ref_cnt() {
                panic!(
                    "can't unlock w_lock_flag {} r_ref_cnt {}",
                    old_v.w_lock_flag(),
                    old_v.r_ref_cnt()
                );
            } else if self.atomic_cxchg_atomic_v(old_v.v(), new_v.v()) {
                break;
            } else {
                util::pause();
            }
        }
    }

    pub unsafe fn rlock_guard(&mut self) -> RLockGuard {
        self.rlock();
        RLockGuard::new(self)
    }

    pub unsafe fn wlock_guard(&mut self) -> WLockGuard {
        self.lock();
        WLockGuard::new(self)
    }
}

impl Default for SpinRWLock {
    fn default() -> Self {
        SpinRWLock {
            atomic_info: Default::default(),
            w_owner: 0,
        }
    }
}

pub struct RLockGuard {
    lock: *mut SpinRWLock,
}

impl RLockGuard {
    unsafe fn destroy(&mut self) {
        if !self.lock.is_null() {
            (*self.lock).unrlock();
            self.lock = ptr::null_mut();
        }
    }

    pub fn new(lock: *mut SpinRWLock) -> Self {
        RLockGuard { lock }
    }
}

impl Default for RLockGuard {
    fn default() -> Self {
        RLockGuard {
            lock: ptr::null_mut(),
        }
    }
}

pub struct WLockGuard {
    lock: *mut SpinRWLock,
}

impl WLockGuard {
    unsafe fn destroy(&mut self) {
        if !self.lock.is_null() {
            (*self.lock).unlock();
            self.lock = ptr::null_mut();
        }
    }

    pub fn new(lock: *mut SpinRWLock) -> Self {
        WLockGuard { lock }
    }
}

impl Default for WLockGuard {
    fn default() -> Self {
        WLockGuard {
            lock: ptr::null_mut(),
        }
    }
}

mod test {
    #[test]
    fn test_rwlock() {
        use spin_rwlock::SpinRWLock;
        let mut lock = SpinRWLock::default();
        assert_eq!(lock.atomic_info.r_ref_cnt(), 0);
        assert!(lock.try_rlock());
        assert_eq!(lock.atomic_info.r_ref_cnt(), 1);
        lock.rlock();
        assert_eq!(lock.atomic_info.r_ref_cnt(), 2);
        assert!(!lock.try_lock());
        assert!(!lock.try_lock());
        assert_eq!(lock.atomic_info.r_ref_cnt(), 2);
        unsafe {
            lock.unrlock();
        }
        unsafe {
            lock.unrlock();
        }
        assert_eq!(lock.atomic_info.r_ref_cnt(), 0);
        lock.lock();
        assert!(!lock.try_lock());
        assert!(!lock.try_rlock());
        assert_eq!(lock.atomic_info.w_pending(), 0);
        assert_eq!(lock.atomic_info.w_lock_flag(), 1);
        unsafe {
            lock.unlock();
        }
        assert_eq!(lock.atomic_info.w_lock_flag(), 0);
        assert_eq!(lock.atomic_info.r_ref_cnt(), 0);
    }
}
