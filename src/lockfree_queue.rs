use hazard_epoch::HazardEpoch;
use hazard_pointer::{BaseHazardNode, HazardNodeI};
use util;
use std::ptr;
use std::intrinsics;
//use std::alloc::{Alloc, Global, Layout};
//use std::mem;

type FIFONodePtr<T> = *mut FIFONode<T>;

struct FIFONode<T> {
    v: Option<T>,
    base: BaseHazardNode,
    next: FIFONodePtr<T>,
}

impl<T> HazardNodeI for FIFONode<T> {
    fn get_base_hazard_node(&self) -> *mut BaseHazardNode {
        &self.base as *const _ as *mut _
    }
}

impl<T> Drop for FIFONode<T> {
    fn drop(&mut self) {}
}

impl<T> Default for FIFONode<T> {
    fn default() -> Self {
        FIFONode {
            v: None,
            base: BaseHazardNode::default(),
            next: ptr::null_mut(),
        }
    }
}

impl<T> FIFONode<T> {
    fn get(&mut self) -> Option<T> {
        self.v.take()
    }

    fn next(&self) -> FIFONodePtr<T> {
        self.next
    }

    fn set_next(&mut self, next: FIFONodePtr<T>) {
        self.next = next;
    }
}

struct LockFreeQueue<T> {
    hazard_epoch: HazardEpoch,
    head: util::WrappedAlign64Type<FIFONodePtr<T>>,
    tail: util::WrappedAlign64Type<FIFONodePtr<T>>,
}

impl<T> LockFreeQueue<T> {
    unsafe fn atomic_load_head(&self) -> FIFONodePtr<T> {
        intrinsics::atomic_load(&*self.head)
    }

    unsafe fn atomic_load_tail(&self) -> FIFONodePtr<T> {
        intrinsics::atomic_load(&*self.tail)
    }

    fn new() -> Self {
        LockFreeQueue {
            hazard_epoch: HazardEpoch::default(),
            head: util::WrappedAlign64Type(ptr::null_mut()),
            tail: util::WrappedAlign64Type(ptr::null_mut()),
        }
    }
}

mod test {
    //    use lockfree_queue::LockFreeQueue;
    //    use std::ptr;
    //    use std::mem;

    #[test]
    fn test_base() {}
}
