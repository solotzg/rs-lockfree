//! Definition and implementations of of `HazardEpoch`
//!
use util::WrappedAlign64Type;
use spin_lock::SpinLock;
use hazard_pointer::{ThreadStore, VersionHandle};
use std::ptr;
use std::mem;
use std::intrinsics;
use util;
use error;
use util::sync_fetch_and_add;
use util::sync_add_and_fetch;

pub use hazard_pointer::{BaseHazardNode, HazardNodeT};

cfg_if! {
    if #[cfg(feature = "max_thread_count_4096")] {
        pub const MAX_THREAD_COUNT: usize = 4096;
    } else if #[cfg(feature = "max_thread_count_256")] {
        pub const MAX_THREAD_COUNT: usize = 256;
    } else {
        /// Maximum thread count
        pub const MAX_THREAD_COUNT: usize = 16;
    }
}

struct VersionTimestamp {
    curr_min_version: u64,
    curr_min_version_timestamp: i64,
}

/// `HazardEpoch` a practical implementation of `Hazard Pointers`, which use global incremental
/// version to identify shared object to be reclaimed. Because of [`False sharing`](https://en.wikipedia.org/wiki/False_sharing),
/// a part of the member variables, might be frequently modified by different threads, are aligned
/// to 64 bytes.
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
    #[inline]
    unsafe fn curr_min_version(&self) -> u64 {
        intrinsics::atomic_load(&self.curr_min_version_info.curr_min_version)
    }

    #[inline]
    unsafe fn set_curr_min_version(&mut self, curr_min_version: u64) {
        intrinsics::atomic_store(
            &mut self.curr_min_version_info.curr_min_version,
            curr_min_version,
        );
    }

    #[inline]
    unsafe fn curr_min_version_timestamp(&self) -> i64 {
        intrinsics::atomic_load(&self.curr_min_version_info.curr_min_version_timestamp)
    }

    #[inline]
    unsafe fn set_curr_min_version_timestamp(&mut self, curr_min_version_timestamp: i64) {
        intrinsics::atomic_store(
            &mut self.curr_min_version_info.curr_min_version_timestamp,
            curr_min_version_timestamp,
        );
    }

    /// To improve performance, `HazardEpoch` can be allocated in stack directly, but it can't be
    /// moved after calling any method. `thread_waiting_threshold` means the maximum of the number of
    /// shared objects to be reclaimed under one thread. `min_version_cache_time_us` means the time
    /// interval(microsecond) to update minimum version cache.
    ///
    /// # Examples
    ///
    /// ```
    /// use rs_lockfree::hazard_epoch::HazardEpoch;
    ///
    /// let h = unsafe { HazardEpoch::new_in_stack(64, 200000) };
    /// let addr_h = &h as *const _ as usize;
    /// assert_eq!(addr_h % 64, 0);
    /// ```
    ///
    #[inline]
    pub unsafe fn new_in_stack(
        thread_waiting_threshold: i64,
        min_version_cache_time_us: i64,
    ) -> HazardEpoch {
        let mut ret = HazardEpoch {
            thread_waiting_threshold,
            min_version_cache_time_us,
            version: WrappedAlign64Type(0),
            thread_lock: WrappedAlign64Type(SpinLock::default()),
            threads: mem::zeroed(),
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

    /// Alloc `HazardEpoch` in heap. Usage is the same as `new_in_stack`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rs_lockfree::hazard_epoch::HazardEpoch;
    ///
    /// let h = HazardEpoch::new_in_heap(64, 200000);
    /// let _addr_h = &h as *const _ as usize;
    /// ```
    ///
    #[inline]
    pub fn new_in_heap(thread_waiting_threshold: i64, min_version_cache_time_us: i64) -> Box<Self> {
        unsafe {
            Box::new(Self::new_in_stack(
                thread_waiting_threshold,
                min_version_cache_time_us,
            ))
        }
    }

    /// Return `Self::new_in_stack(64, 200000)`
    #[inline]
    pub unsafe fn default_new_in_stack() -> Self {
        Self::new_in_stack(64, 200000)
    }

    /// Return `Self::new_in_heap(64, 200000)`
    #[inline]
    pub fn default_new_in_heap() -> Box<Self> {
        Self::new_in_heap(64, 200000)
    }

    #[inline]
    unsafe fn destroy(&mut self) {
        self.retire();
    }

    /// Reclaim all shared objects waiting to be reclaimed. It will be called when dropping `HazardEpoch`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rs_lockfree::hazard_epoch::HazardEpoch;
    /// use rs_lockfree::hazard_epoch::BaseHazardNode;
    ///
    /// let mut h = HazardEpoch::new_in_heap(64, 200000);
    /// let node = Box::into_raw(Box::new(BaseHazardNode::default()));
    /// unsafe { h.add_node(node); }
    /// unsafe { h.retire(); }
    /// ```
    ///
    pub unsafe fn retire(&mut self) {
        let mut ts = ptr::null_mut::<ThreadStore>();
        let ret = self.get_thread_store(&mut ts);
        if ret != error::Status::Success {
            warn!("get_thread_store fail, ret={}", ret);
            return;
        }
        let min_version = self.get_min_version(true);
        let retire_count = (*ts).retire(min_version, &mut *ts);
        sync_fetch_and_add(self.hazard_waiting_count.as_mut_ptr(), -retire_count);

        let mut iter = self.atomic_load_thread_list();
        while !iter.is_null() {
            if iter != ts {
                let retire_count = (*iter).retire(min_version, &mut *ts);
                sync_fetch_and_add(self.hazard_waiting_count.as_mut_ptr(), -retire_count);
            }
            iter = (*iter).next();
        }
    }

    /// Reclaim all shared objects waiting to be reclaimed. `node` can be any type as long as it implements
    /// Trait `HazardNodeT`. `BaseHazardNode` is used to realize `vtable`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rs_lockfree::hazard_epoch::HazardEpoch;
    /// use rs_lockfree::hazard_epoch::{BaseHazardNode, HazardNodeT};
    /// use std::cell::RefCell;
    ///
    /// struct Node<'a, T> {
    ///     base: BaseHazardNode,
    ///     cnt: &'a RefCell<i32>,
    ///     v: T,
    /// }
    ///
    /// impl<'a, T> Drop for Node<'a, T> {
    ///     fn drop(&mut self) {
    ///         *self.cnt.borrow_mut() += 10;
    ///     }
    /// }
    ///
    /// impl<'a, T> HazardNodeT for Node<'a, T> {
    ///     fn get_base_hazard_node(&self) -> *mut BaseHazardNode {
    ///         &self.base as *const _ as *mut _
    ///     }
    /// }
    ///
    /// let cnt = RefCell::new(0);
    /// let mut h = HazardEpoch::default_new_in_heap();
    /// let node = Box::into_raw(Box::new(Node{
    ///     base: Default::default(),
    ///     cnt: &cnt,
    ///     v: 2333,
    /// }));
    /// unsafe { h.add_node(node); }
    /// drop(h);
    /// assert_eq!(*cnt.borrow(), 10);
    /// ```
    ///
    #[inline]
    pub unsafe fn add_node<T>(&mut self, node: *mut T) -> error::Status
    where
        T: HazardNodeT,
    {
        let mut ts = ptr::null_mut::<ThreadStore>();
        let mut ret;
        if node.is_null() {
            warn!("node is null");
            ret = error::Status::InvalidParam;
        } else if error::Status::Success != {
            ret = self.get_thread_store(&mut ts);
            ret
        } {
            warn!("get_thread_store fail, ret={}", ret);
        } else if error::Status::Success != {
            ret = (*ts).add_node(sync_add_and_fetch(self.version.as_mut_ptr(), 1), node);
            ret
        } {
            warn!("add_node fail, ret={}", ret);
        } else {
            sync_fetch_and_add(self.hazard_waiting_count.as_mut_ptr(), 1);
        }
        ret
    }

    #[inline]
    fn atomic_load_version(&self) -> u64 {
        unsafe { intrinsics::atomic_load(self.version.as_ptr()) }
    }

    /// Before accessing a shared object, call method `acquire` to get the `handle` of this operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use rs_lockfree::hazard_epoch::HazardEpoch;
    /// use rs_lockfree::hazard_epoch::BaseHazardNode;
    /// use rs_lockfree::error::Status;
    ///
    /// let mut h = HazardEpoch::default_new_in_heap();
    /// let node = Box::into_raw(Box::new(BaseHazardNode::default()));
    /// let mut handle = 0;
    /// assert_eq!(h.acquire(&mut handle), Status::Success);
    /// let _o = unsafe { &(*node) };
    /// unsafe { h.release(handle); }
    /// ```
    ///
    pub fn acquire(&mut self, handle: &mut u64) -> error::Status {
        let mut ts = ptr::null_mut::<ThreadStore>();
        let mut ret;
        if error::Status::Success != {
            ret = unsafe { self.get_thread_store(&mut ts) };
            ret
        } {
            warn!("get_thread_store fail, ret={}", ret);
        } else {
            let ts = unsafe { &mut *ts };
            loop {
                let version = self.atomic_load_version();
                let mut version_handle = VersionHandle::new(0);
                if error::Status::Success != {
                    ret = ts.acquire(version, &mut version_handle);
                    ret
                } {
                    warn!("thread store acquire fail, ret={}", ret);
                    break;
                } else if version != self.atomic_load_version() {
                    ts.release(&version_handle);
                } else {
                    *handle = version_handle.ver_u64();
                    break;
                }
            }
        }
        ret
    }

    /// Atomic load count of thread
    #[inline]
    fn atomic_load_thread_count(&self) -> i64 {
        unsafe { intrinsics::atomic_load(&self.thread_count) }
    }

    /// After accessing a shared object, call method `release` to trigger reclaiming. Usage is the
    /// same as `acquire`.
    #[inline]
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
                sync_fetch_and_add(self.hazard_waiting_count.as_mut_ptr(), -retire_count);
            } else if self.atomic_load_thread_count() * self.thread_waiting_threshold
                < self.atomic_load_hazard_waiting_count()
            {
                self.retire();
            }
        }
    }

    /// Atomic load count of shared objects waiting to be reclaimed.
    #[inline]
    pub fn atomic_load_hazard_waiting_count(&self) -> i64 {
        unsafe { intrinsics::atomic_load(self.hazard_waiting_count.as_ptr()) }
    }

    #[inline]
    unsafe fn get_thread_store(&mut self, ts: &mut *mut ThreadStore) -> error::Status {
        let mut ret = error::Status::Success;
        let tn = util::get_thread_id() as u16;
        if MAX_THREAD_COUNT <= tn as usize {
            warn!("thread number overflow, tn={}", tn);
            ret = error::Status::ThreadNumOverflow;
        } else {
            *ts = self.threads.as_mut_ptr().offset(tn as isize);
            let ts_obj = &mut **ts;
            // different thread use different thread store.
            if !ts_obj.is_enabled() {
                // CAS can be used directly here, no ABA problem.
                // Atomicity of thread_count is not necessary.

                self.thread_lock.lock();

                ts_obj.set_enabled(tn);
                ts_obj.set_next(self.atomic_load_thread_list());
                intrinsics::atomic_store(
                    &mut self.thread_list as *mut _ as *mut usize,
                    *ts as usize,
                );
                sync_fetch_and_add(&mut self.thread_count, 1);

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
        let mut ret = 0;
        if !force_flush && 0 != {
            ret = self.curr_min_version();
            ret
        }
            && self.curr_min_version_timestamp() + self.min_version_cache_time_us
                > util::get_cur_microseconds_time()
        {
        } else {
            ret = self.atomic_load_version();
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

impl Drop for HazardEpoch {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}
