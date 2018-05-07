use hazard_epoch::HazardEpoch;
use hazard_pointer::{BaseHazardNode, HazardNodeI};
use util;
use std::ptr;

type FIFONodePtr<T> = *mut FIFONode<T>;

struct FIFONode<T> {
    value: Option<T>,
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
            value: None,
            base: BaseHazardNode::default(),
            next: ptr::null_mut(),
        }
    }
}

impl<T> FIFONode<T> {
    fn next(&self) -> FIFONodePtr<T> {
        self.next
    }

    fn set_next(&mut self, next: FIFONodePtr<T>) {
        self.next = next;
    }

    fn new(value: T) -> Self {
        FIFONode {
            value: Some(value),
            base: BaseHazardNode::default(),
            next: ptr::null_mut(),
        }
    }
}

struct LockFreeQueue<T> {
    hazard_epoch: HazardEpoch,
    head: util::WrappedAlign64Type<FIFONodePtr<T>>,
    tail: util::WrappedAlign64Type<FIFONodePtr<T>>,
}

impl<T> LockFreeQueue<T> {
    unsafe fn atomic_load_head(&self) -> FIFONodePtr<T> {
        util::atomic_load_raw_ptr(&*self.head)
    }

    unsafe fn atomic_load_tail(&self) -> FIFONodePtr<T> {
        util::atomic_load_raw_ptr(&*self.tail)
    }

    fn new() -> LockFreeQueue<T> {
        let head = Box::into_raw(Box::new(FIFONode::<T>::default()));
        LockFreeQueue {
            hazard_epoch: HazardEpoch::default(),
            head: util::WrappedAlign64Type(head),
            tail: util::WrappedAlign64Type(head),
        }
    }

    pub fn push(&mut self, v: T) {
        unsafe { self.inner_push(v) }
    }

    unsafe fn inner_push(&mut self, v: T) {
        let node = Box::into_raw(Box::new(FIFONode::new(v)));
        let mut handle = 0_u64;
        self.hazard_epoch.acquire(&mut handle);
        let mut cur = self.atomic_load_tail();
        let mut old = cur;
        while !{
            let (tmp, b) = util::atomic_cxchg_raw_ptr(&mut *self.tail, old, node);
            cur = tmp;
            b
        } {
            old = cur;
        }
        (*cur).set_next(node);
        self.hazard_epoch.release(handle);
    }

    pub fn pop(&mut self) -> Option<T> {
        unsafe { self.inner_pop() }
    }

    unsafe fn inner_pop(&mut self) -> Option<T> {
        let mut ret = None;
        let mut handle = 0_u64;
        self.hazard_epoch.acquire(&mut handle);
        let mut cur = self.atomic_load_head();
        let mut old = cur;
        let mut node = (*cur).next();
        while !node.is_null() && !{
            let (tmp, b) = util::atomic_cxchg_raw_ptr(&mut *self.head, old, node);
            cur = tmp;
            b
        } {
            old = cur;
            node = (*cur).next();
        }
        if !node.is_null() {
            ret = (*node).value.take();
            assert!(ret.is_some());
            self.hazard_epoch.add_node(cur);
        }
        self.hazard_epoch.release(handle);
        ret
    }

    pub unsafe fn destroy(&mut self) {
        let mut head = *self.head;
        while !head.is_null() {
            head = Box::from_raw(head).next;
        }
        self.head = util::WrappedAlign64Type(ptr::null_mut());
        self.tail = util::WrappedAlign64Type(ptr::null_mut());
    }
}

impl<T> Drop for LockFreeQueue<T> {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}

mod test {
    use lockfree_queue::LockFreeQueue;
    use std::cell::RefCell;

    struct Node<'a, T> {
        cnt: &'a RefCell<i32>,
        v: T,
    }

    impl<'a, T> Drop for Node<'a, T> {
        fn drop(&mut self) {
            *self.cnt.borrow_mut() += 1;
        }
    }

    #[test]
    fn test_base() {
        let mut queue = LockFreeQueue::new();
        assert!(queue.pop().is_none());
        queue.push(1);
        assert_eq!(queue.pop().unwrap(), 1);
        let test_num = 100;
        for i in 0..test_num {
            queue.push(i);
        }
        for i in 0..test_num {
            assert_eq!(queue.pop().unwrap(), i);
        }
    }

    #[test]
    fn test_memory_leak() {
        let cnt = RefCell::new(0);
        let mut queue = LockFreeQueue::new();
        let test_num = 100;
        for i in 0..test_num {
            queue.push(Node { cnt: &cnt, v: i });
        }
        unsafe {
            assert!((**queue.head).value.is_none());
        }
        assert_eq!(*cnt.borrow(), 0);
        for i in 0..test_num {
            assert_eq!(queue.pop().unwrap().v, i);
        }
        assert_eq!(*cnt.borrow(), test_num);
    }
}
