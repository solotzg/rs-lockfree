use util::WrappedAlign64Type;
use spin_lock::SpinLock;
use hazard_pointer::{ThreadStore, VersionHandle};
use std::ptr;
use std::mem;
use hazard_pointer::HazardNodeI;
use std::intrinsics;
use util;
use error;
use util::sync_fetch_and_add;
use util::sync_add_and_fetch;

const MAX_THREAD_COUNT: usize = 4096;

struct VersionTimestamp {
    curr_min_version: u64,
    curr_min_version_timestamp: i64,
}

pub struct HazardEpoch {
    thread_waiting_threshold: i64,
    min_version_cache_time_us: i64,
    version: WrappedAlign64Type<u64>,
    thread_lock: WrappedAlign64Type<SpinLock>,
    threads: [ThreadStore; MAX_THREAD_COUNT],
    thread_list: *mut ThreadStore,
    thread_count: i64,
    hazard_waiting_count: WrappedAlign64Type<i64>,
    curr_min_version_info: WrappedAlign64Type<VersionTimestamp>,
}

impl HazardEpoch {
    unsafe fn curr_min_version(&self) -> u64 {
        intrinsics::atomic_load(&self.curr_min_version_info.curr_min_version)
    }

    unsafe fn set_curr_min_version(&mut self, curr_min_version: u64) {
        intrinsics::atomic_store(
            &mut self.curr_min_version_info.curr_min_version,
            curr_min_version,
        );
    }

    unsafe fn curr_min_version_timestamp(&self) -> i64 {
        intrinsics::atomic_load(&self.curr_min_version_info.curr_min_version_timestamp)
    }

    unsafe fn set_curr_min_version_timestamp(&mut self, curr_min_version_timestamp: i64) {
        intrinsics::atomic_store(
            &mut self.curr_min_version_info.curr_min_version_timestamp,
            curr_min_version_timestamp,
        );
    }

    pub fn new(thread_waiting_threshold: i64, min_version_cache_time_us: i64) -> HazardEpoch {
        let mut ret = HazardEpoch {
            thread_waiting_threshold,
            min_version_cache_time_us,
            version: WrappedAlign64Type(0),
            thread_lock: WrappedAlign64Type(SpinLock::default()),
            threads: unsafe { mem::zeroed() },
            thread_list: ptr::null_mut(),
            thread_count: 0,
            hazard_waiting_count: WrappedAlign64Type(0),
            curr_min_version_info: WrappedAlign64Type(VersionTimestamp {
                curr_min_version: 0,
                curr_min_version_timestamp: 0,
            }),
        };
        for idx in 0..ret.threads.len() {
            ret.threads[idx] = ThreadStore::default();
        }
        ret
    }

    unsafe fn destroy(&mut self) {
        self.retire();
    }

    pub unsafe fn retire(&mut self) {
        let mut ts = ptr::null_mut::<ThreadStore>();
        let ret = self.get_thread_store(&mut ts);
        if ret != error::Status::Success {
            warn!("get_thread_store_ fail, ret={}", ret);
            return;
        }
        let min_version = self.get_min_version(true);
        let retire_count = (*ts).retire(min_version, &mut *ts);
        sync_fetch_and_add(&mut *self.hazard_waiting_count, -retire_count);

        let mut iter = self.atomic_load_thread_list();
        while !iter.is_null() {
            if iter != ts {
                let retire_count = (*iter).retire(min_version, &mut *ts);
                sync_fetch_and_add(&mut *self.hazard_waiting_count, -retire_count);
            }
            iter = (*iter).next();
        }
    }

    pub unsafe fn add_node<T>(&mut self, node: *mut T) -> error::Status
    where
        T: HazardNodeI,
    {
        let mut ts = ptr::null_mut::<ThreadStore>();
        if node.is_null() {
            warn!("invalid param, node null pointer");
            return error::Status::InvalidParam;
        }
        let mut ret = self.get_thread_store(&mut ts);
        if ret != error::Status::Success {
            warn!("get_thread_store_ fail, ret={}", ret);
            return ret;
        }
        ret = (*ts).add_node(sync_add_and_fetch(&mut *self.version, 1), node);
        if ret != error::Status::Success {
            warn!("add_node fail, ret={}", ret);
            return ret;
        }
        sync_fetch_and_add(&mut *self.hazard_waiting_count, 1);
        ret
    }

    unsafe fn version(&self) -> u64 {
        intrinsics::atomic_load(&*self.version)
    }

    unsafe fn add_version(&mut self, add: u64) -> u64 {
        sync_fetch_and_add(&mut *self.version, add)
    }

    pub unsafe fn acquire(&mut self, handle: &mut u64) -> error::Status {
        let mut ts = ptr::null_mut::<ThreadStore>();
        let mut ret = self.get_thread_store(&mut ts);
        if ret != error::Status::Success {
            warn!("get_thread_store fail, ret={}", ret);
            return ret;
        }
        loop {
            let version = intrinsics::atomic_load(&*self.version);
            let mut version_handle = VersionHandle::new(0);
            ret = (*ts).acquire(version, &mut version_handle);
            if ret != error::Status::Success {
                warn!("thread store acquire fail, ret={}", ret);
                break;
            } else if version != intrinsics::atomic_load(&*self.version) {
                (*ts).release(&version_handle);
            } else {
                *handle = version_handle.ver_u64();
                break;
            }
        }
        ret
    }

    unsafe fn atomic_load_thread_count(&self) -> i64 {
        intrinsics::atomic_load(&self.thread_count)
    }

    pub unsafe fn release(&mut self, handle: u64) {
        let version_handle = VersionHandle::new(handle);
        if MAX_THREAD_COUNT > version_handle.tid() as usize {
            let ts = self.threads
                .as_mut_ptr()
                .offset(version_handle.tid() as isize);
            (*ts).release(&version_handle);
            if self.thread_waiting_threshold < (*ts).get_hazard_waiting_count() {
                let min_version = self.get_min_version(false);
                let retire_count = (*ts).retire(min_version, &mut *ts);
                sync_fetch_and_add(&mut *self.hazard_waiting_count, -retire_count);
            } else if self.atomic_load_thread_count() * self.thread_waiting_threshold
                < self.get_hazard_waiting_count()
            {
                self.retire();
            }
        }
    }

    pub unsafe fn get_hazard_waiting_count(&self) -> i64 {
        intrinsics::atomic_load(&*self.hazard_waiting_count)
    }

    unsafe fn get_thread_store(&mut self, ts: &mut *mut ThreadStore) -> error::Status {
        let mut ret = error::Status::Success;
        let tn = util::get_thread_id() as u16;
        if MAX_THREAD_COUNT <= tn as usize {
            warn!("number overflow, tn={}", tn);
            ret = error::Status::TooManyThreads;
        } else {
            *ts = self.threads.as_mut_ptr().offset(tn as isize);
            if !(**ts).is_enabled() {
                self.thread_lock.lock();
                if !(**ts).is_enabled() {
                    (**ts).set_enabled(tn);
                    (**ts).set_next(self.atomic_load_thread_list());
                    intrinsics::atomic_store(
                        &mut self.thread_list as *mut _ as *mut usize,
                        *ts as usize,
                    );
                    sync_fetch_and_add(&mut self.thread_count, 1);
                }
                self.thread_lock.unlock();
            }
        }
        ret
    }

    #[inline]
    unsafe fn atomic_load_thread_list(&self) -> *mut ThreadStore {
        util::atomic_load_raw_ptr(&self.thread_list)
    }

    unsafe fn get_min_version(&mut self, force_flush: bool) -> u64 {
        let mut ret = self.curr_min_version();
        if !force_flush && ret != 0
            && self.curr_min_version_timestamp() + self.min_version_cache_time_us
                > util::get_cur_microseconds_time()
        {
        } else {
            ret = self.version();
            let mut iter = self.atomic_load_thread_list();
            while !iter.is_null() {
                let ts_min_version = (*iter).version();
                if ret > ts_min_version {
                    ret = ts_min_version;
                }
                iter = (*iter).next();
            }
            self.set_curr_min_version(ret);
            self.set_curr_min_version_timestamp(util::get_cur_microseconds_time());
        }
        ret
    }
}

impl Default for HazardEpoch {
    fn default() -> Self {
        HazardEpoch::new(64, 200000)
    }
}

impl Drop for HazardEpoch {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}
