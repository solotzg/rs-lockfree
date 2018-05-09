use hazard_epoch::HazardEpoch;
use hazard_pointer::{BaseHazardNode, HazardNodeI};
use util;
use std::ptr;

type LIFONodePtr<T> = *mut LIFONode<T>;

struct LIFONode<T> {
    value: Option<T>,
    base: BaseHazardNode,
    next: LIFONodePtr<T>,
}

impl<T> HazardNodeI for LIFONode<T> {
    fn get_base_hazard_node(&self) -> *mut BaseHazardNode {
        &self.base as *const _ as *mut _
    }
}

impl<T> Drop for LIFONode<T> {
    fn drop(&mut self) {}
}

impl<T> Default for LIFONode<T> {
    fn default() -> Self {
        LIFONode {
            value: None,
            base: BaseHazardNode::default(),
            next: ptr::null_mut(),
        }
    }
}

impl<T> LIFONode<T> {
    fn next(&self) -> LIFONodePtr<T> {
        self.next
    }

    fn set_next(&mut self, next: LIFONodePtr<T>) {
        self.next = next;
    }

    fn new(value: T) -> Self {
        LIFONode {
            value: Some(value),
            base: BaseHazardNode::default(),
            next: ptr::null_mut(),
        }
    }
}

pub struct LockFreeStack<T> {
    hazard_epoch: HazardEpoch,
    top: util::WrappedAlign64Type<LIFONodePtr<T>>,
}

impl<T> LockFreeStack<T> {
    unsafe fn atomic_load_top(&self) -> LIFONodePtr<T> {
        util::atomic_load_raw_ptr(&*self.top)
    }

    pub unsafe fn default_new_in_stack() -> LockFreeStack<T> {
        LockFreeStack {
            hazard_epoch: HazardEpoch::default_new_in_stack(),
            top: util::WrappedAlign64Type(ptr::null_mut()),
        }
    }

    pub fn default_new_in_heap() -> Box<Self> {
        unsafe { Box::new(Self::default_new_in_stack()) }
    }

    pub fn push(&mut self, v: T) {
        unsafe { self.inner_push(v) }
    }

    unsafe fn inner_push(&mut self, v: T) {
        let node = Box::into_raw(Box::new(LIFONode::new(v)));
        let mut handle = 0_u64;
        self.hazard_epoch.acquire(&mut handle);
        let mut cur = self.atomic_load_top();
        let mut old = cur;
        (*node).set_next(old);
        while !{
            let (tmp, b) = util::atomic_cxchg_raw_ptr(&mut *self.top, old, node);
            cur = tmp;
            b
        } {
            old = cur;
            (*node).set_next(old);
        }
        self.hazard_epoch.release(handle);
    }

    pub fn pop(&mut self) -> Option<T> {
        unsafe { self.inner_pop() }
    }

    unsafe fn inner_pop(&mut self) -> Option<T> {
        let mut ret = None;
        let mut handle = 0_u64;
        self.hazard_epoch.acquire(&mut handle);
        let mut cur = self.atomic_load_top();
        let mut old = cur;
        while !cur.is_null() && !{
            let (tmp, b) = util::atomic_cxchg_raw_ptr(&mut *self.top, old, (*cur).next());
            cur = tmp;
            b
        } {
            old = cur;
        }
        if !cur.is_null() {
            ret = (*cur).value.take();
            assert!(ret.is_some());
            self.hazard_epoch.add_node(cur);
        }
        self.hazard_epoch.release(handle);
        ret
    }

    pub unsafe fn destroy(&mut self) {
        let mut head = *self.top;
        while !head.is_null() {
            head = Box::from_raw(head).next;
        }
        self.top = util::WrappedAlign64Type(ptr::null_mut());
    }
}

impl<T> Drop for LockFreeStack<T> {
    fn drop(&mut self) {
        unsafe {
            self.destroy();
        }
    }
}

mod test {
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
        use lockfree_stack::LockFreeStack;
        let mut queue = unsafe { LockFreeStack::default_new_in_stack() };
        assert!(queue.pop().is_none());
        queue.push(1);
        assert_eq!(queue.pop().unwrap(), 1);
        let test_num = 100;
        for i in 0..test_num {
            queue.push(i);
        }
        for i in 0..test_num {
            assert_eq!(queue.pop().unwrap(), test_num - i - 1);
        }
    }

    #[test]
    fn test_memory_leak() {
        use lockfree_stack::LockFreeStack;
        let cnt = RefCell::new(0);
        let mut queue = unsafe { LockFreeStack::default_new_in_stack() };
        let test_num = 100;
        for i in 0..test_num {
            queue.push(Node { cnt: &cnt, v: i });
        }
        assert_eq!(*cnt.borrow(), 0);
        for i in 0..test_num {
            assert_eq!(queue.pop().unwrap().v, test_num - i - 1);
        }
        assert_eq!(*cnt.borrow(), test_num);
    }
}
