use std::ptr;
use util;
use error;
use std;
use std::intrinsics;
use std::{mem, raw};
use util::WrappedAlign64Type;
use util::sync_fetch_and_add;

struct SeqVersion {
    seq: u32,
    version: u64,
}

impl Default for SeqVersion {
    fn default() -> Self {
        SeqVersion {
            seq: 0,
            version: std::u64::MAX,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TidSeq {
    tid: u16,
    high_bits: u16,
    seq: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
union VersionHandleUnion {
    tid_seq: TidSeq,
    ver_u64: u64,
}

#[derive(Copy, Clone)]
pub struct VersionHandle {
    data: VersionHandleUnion,
}

impl VersionHandle {
    #[inline]
    pub fn ver_u64(&self) -> u64 {
        unsafe { self.data.ver_u64 }
    }

    #[inline]
    pub fn new(uv: u64) -> VersionHandle {
        VersionHandle {
            data: VersionHandleUnion { ver_u64: uv },
        }
    }

    #[inline]
    fn set_tid(&mut self, tid: u16) {
        unsafe {
            self.data.tid_seq.tid = tid;
        }
    }

    #[inline]
    pub fn tid(&self) -> u16 {
        unsafe { self.data.tid_seq.tid }
    }

    #[inline]
    fn set_high_bits(&mut self, high_bits: u16) {
        unsafe {
            self.data.tid_seq.high_bits = high_bits;
        }
    }

    #[inline]
    fn seq(&self) -> u32 {
        unsafe { self.data.tid_seq.seq }
    }

    #[inline]
    fn set_seq(&mut self, seq: u32) {
        unsafe {
            self.data.tid_seq.seq = seq;
        }
    }
}

/// Trait `HazardNodeT` is used to achieve `virtual function`.
pub trait HazardNodeT: Drop {
    /// It's necessary to put `BaseHazardNode`, which can be accessed by method `get_base_hazard_node`,
    /// in custom struct.
    ///
    /// # Examples
    /// ```
    /// use rs_lockfree::hazard_epoch::{BaseHazardNode, HazardNodeT, HazardEpoch};
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
    /// ```
    fn get_base_hazard_node(&self) -> *mut BaseHazardNode;
}

/// Definition ans usage is shown in [`HazardNodeT`]
///
/// [`HazardNodeT`]: trait.HazardNodeT.html
///
pub struct BaseHazardNode {
    trait_obj: raw::TraitObject,
    next: *mut BaseHazardNode,
    version: u64,
}

impl Default for BaseHazardNode {
    fn default() -> Self {
        BaseHazardNode {
            trait_obj: unsafe { mem::zeroed() },
            next: ptr::null_mut(),
            version: std::u64::MAX,
        }
    }
}

impl HazardNodeT for BaseHazardNode {
    fn get_base_hazard_node(&self) -> *mut BaseHazardNode {
        self as *const _ as *mut BaseHazardNode
    }
}

impl Drop for BaseHazardNode {
    fn drop(&mut self) {}
}

impl BaseHazardNode {
    #[inline]
    fn next(&self) -> *mut BaseHazardNode {
        self.next
    }

    #[inline]
    fn version(&self) -> u64 {
        self.version
    }

    #[inline]
    fn set_version(&mut self, version: u64) {
        self.version = version;
    }

    #[inline]
    fn set_next(&mut self, next: *mut BaseHazardNode) {
        assert_ne!(next, self as *mut _);
        self.next = next;
    }

    #[inline]
    fn set_tait_obj(&mut self, trait_obj: raw::TraitObject) {
        self.trait_obj = trait_obj;
    }

    #[inline]
    fn trait_obj(&self) -> raw::TraitObject {
        self.trait_obj
    }
}

pub struct ThreadStore {
    enabled: bool,
    tid: u16,
    last_retire_version: u64,
    curr_seq_version: WrappedAlign64Type<SeqVersion>,
    hazard_waiting_list: WrappedAlign64Type<*mut BaseHazardNode>,
    hazard_waiting_count: WrappedAlign64Type<i64>,
    next: WrappedAlign64Type<*mut ThreadStore>,
}

impl Default for ThreadStore {
    fn default() -> Self {
        ThreadStore::new()
    }
}

impl ThreadStore {
    fn new() -> ThreadStore {
        ThreadStore {
            enabled: false,
            tid: 0,
            last_retire_version: 0,
            curr_seq_version: Default::default(),
            hazard_waiting_list: WrappedAlign64Type(ptr::null_mut()),
            hazard_waiting_count: Default::default(),
            next: WrappedAlign64Type(ptr::null_mut()),
        }
    }

    #[inline]
    pub fn set_enabled(&mut self, tid: u16) {
        self.enabled = true;
        self.tid = tid;
    }

    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    fn tid(&self) -> u16 {
        self.tid
    }

    #[inline]
    pub fn set_next(&mut self, next: *mut ThreadStore) {
        self.next = WrappedAlign64Type(next);
    }

    #[inline]
    pub fn next(&self) -> *mut ThreadStore {
        *self.next
    }

    #[inline]
    fn curr_seq(&self) -> u32 {
        self.curr_seq_version.seq
    }

    #[inline]
    fn inc_curr_seq(&mut self) {
        self.curr_seq_version.seq += 1;
    }

    #[inline]
    fn curr_version(&self) -> u64 {
        self.curr_seq_version.version
    }

    #[inline]
    fn set_curr_version(&mut self, version: u64) {
        self.curr_seq_version.version = version;
    }

    #[inline]
    pub fn acquire(&mut self, version: u64, handle: &mut VersionHandle) -> error::Status {
        assert_eq!(self.tid(), util::get_thread_id() as u16);
        let mut ret = error::Status::Success;
        if std::u64::MAX != self.curr_version() {
            warn!(
                "current thread has already assigned a version handle, seq={}",
                self.curr_seq()
            );
            ret = error::Status::Busy;
        } else {
            self.set_curr_version(version);
            handle.set_tid(self.tid());
            handle.set_high_bits(0);
            handle.set_seq(self.curr_seq());
        }
        ret
    }

    pub fn release(&mut self, handle: &VersionHandle) {
        assert_eq!(self.tid(), util::get_thread_id() as u16);
        if self.tid() != handle.tid() && self.curr_seq() != handle.seq() {
            warn!("invalid handle seq={}, tid={}", handle.seq(), handle.tid());
        } else {
            self.set_curr_version(std::u64::MAX);
            self.inc_curr_seq();
        }
    }

    pub unsafe fn add_node<T>(&mut self, version: u64, node: *mut T) -> error::Status
    where
        T: HazardNodeT,
    {
        assert_eq!(self.tid(), util::get_thread_id() as u16);
        let ret = error::Status::Success;
        let base = (*node).get_base_hazard_node();

        (*base).set_tait_obj(mem::transmute::<_, raw::TraitObject>(
            &mut *node as &mut HazardNodeT,
        ));

        (*base).set_version(version);

        self.inner_add_nodes(base, base, 1);

        ret
    }

    #[inline]
    pub fn get_hazard_waiting_count(&self) -> i64 {
        unsafe { intrinsics::atomic_load(self.hazard_waiting_count.as_ptr()) }
    }

    #[inline]
    unsafe fn atomic_load_hazard_waiting_list(&self) -> *mut BaseHazardNode {
        util::atomic_load_raw_ptr(self.hazard_waiting_list.as_ptr())
    }

    pub unsafe fn retire(&mut self, version: u64, node_receiver: &mut ThreadStore) -> i64 {
        assert!(
            self as *const _ != node_receiver as *const _
                || self.tid() == util::get_thread_id() as u16
        );
        if self.last_retire_version == version {
            return 0;
        }
        self.last_retire_version = version;
        let mut curr = self.atomic_load_hazard_waiting_list();
        let mut old = curr;
        while !{
            let (tmp, ok) = self.atomic_cxchg_hazard_waiting_list(old, ptr::null_mut());
            curr = tmp;
            ok
        } {
            old = curr;
        }
        let mut list_retire = ptr::null_mut();
        let mut move_count = 0i64;
        let mut retire_count = 0i64;
        let mut pseudo_head = BaseHazardNode::default();
        pseudo_head.set_next(curr);
        let mut iter = &mut pseudo_head as *mut BaseHazardNode;
        while !(*iter).next().is_null() {
            if (*(*iter).next()).version() <= version {
                retire_count += 1;
                let tmp = (*iter).next();
                (*iter).set_next((*(*iter).next()).next());

                (*tmp).set_next(list_retire);
                list_retire = tmp;
            } else {
                move_count += 1;
                iter = (*iter).next();
            }
        }
        let mut move_list_tail = ptr::null_mut();
        let move_list_head = pseudo_head.next();
        if !move_list_head.is_null() {
            move_list_tail = iter;
        }
        node_receiver.inner_add_nodes(move_list_head, move_list_tail, move_count);
        sync_fetch_and_add(
            self.hazard_waiting_count.as_mut_ptr(),
            -(move_count + retire_count),
        );
        while !list_retire.is_null() {
            let node_retire = list_retire;
            list_retire = (*list_retire).next();
            Self::retire_hazard_node(node_retire);
        }
        retire_count
    }

    unsafe fn retire_hazard_node(node_retire: *mut BaseHazardNode) {
        let trait_obj = (*node_retire).trait_obj();
        let obj = mem::transmute::<raw::TraitObject, &mut HazardNodeT>(trait_obj);
        Box::from_raw(obj as *mut HazardNodeT);
    }

    #[inline]
    pub fn version(&self) -> u64 {
        self.curr_version()
    }

    unsafe fn atomic_cxchg_hazard_waiting_list(
        &mut self,
        old: *mut BaseHazardNode,
        src: *mut BaseHazardNode,
    ) -> (*mut BaseHazardNode, bool) {
        util::atomic_cxchg_raw_ptr(self.hazard_waiting_list.as_mut_ptr(), old, src)
    }

    unsafe fn inner_add_nodes(
        &mut self,
        head: *mut BaseHazardNode,
        tail: *mut BaseHazardNode,
        count: i64,
    ) {
        assert_eq!(self.tid(), util::get_thread_id() as u16);
        if 0 < count {
            let mut curr = self.atomic_load_hazard_waiting_list();
            let mut old = curr;
            (*tail).set_next(curr);
            while !{
                let (tmp, ok) = self.atomic_cxchg_hazard_waiting_list(old, head);
                curr = tmp;
                ok
            } {
                old = curr;
                (*tail).set_next(old);
            }
            sync_fetch_and_add(self.hazard_waiting_count.as_mut_ptr(), count);
        }
    }

    unsafe fn destroy(&mut self) {
        while !self.hazard_waiting_list.is_null() {
            let node_retire = *self.hazard_waiting_list;
            self.hazard_waiting_list = WrappedAlign64Type((*node_retire).next());
            Self::retire_hazard_node(node_retire);
        }
    }
}

impl Drop for ThreadStore {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}
