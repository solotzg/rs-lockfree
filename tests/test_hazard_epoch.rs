#![feature(core_intrinsics)]

extern crate core_affinity;
extern crate env_logger;
extern crate rs_lockfree;

use std::mem;
use std::thread;
use std::intrinsics;
use std::ops::Deref;
use std::ops::DerefMut;
use std::time;
use rs_lockfree::hazard_pointer::BaseHazardNode;
use rs_lockfree::hazard_pointer::HazardNodeT;
use rs_lockfree::hazard_epoch::HazardEpoch;
use rs_lockfree::util;
use rs_lockfree::error::Status;
use std::ptr;

struct TestObj {
    base: BaseHazardNode,
    cnt: *mut i64,
    data: Option<i32>,
}

impl PartialEq for TestObj {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl HazardNodeT for TestObj {
    fn get_base_hazard_node(&self) -> *mut BaseHazardNode {
        &self.base as *const _ as *mut BaseHazardNode
    }
}

impl Drop for TestObj {
    fn drop(&mut self) {
        self.data.take().unwrap();
        unsafe {
            util::sync_fetch_and_add(self.cnt, -1);
        }
    }
}

impl TestObj {
    fn new(cnt: &mut i64) -> TestObj {
        let o = TestObj {
            base: BaseHazardNode::default(),
            cnt,
            data: Some(0),
        };
        unsafe {
            util::sync_fetch_and_add(o.cnt, 1);
        }
        o
    }
}

struct GlobalConf {
    stop: u8,
    cnt: i64,
    read_loops: i64,
    write_loops: i64,
    v: *mut TestObj,
    h: HazardEpoch,
}

impl GlobalConf {
    unsafe fn set_stop(&mut self, stop: bool) {
        intrinsics::atomic_store(&mut self.stop, stop as u8);
    }

    unsafe fn stop(&self) -> bool {
        intrinsics::atomic_load(&self.stop) != 0
    }
}

fn get_current_tid() -> i64 {
    unsafe { util::get_thread_id() }
}

fn set_cpu_affinity() {
    let cpus = core_affinity::get_core_ids().unwrap();
    core_affinity::set_for_current(cpus[get_current_tid() as usize % cpus.len()]);
    println!(
        "set_cpu_affinity {} {}",
        get_current_tid(),
        get_current_tid() as usize % cpus.len()
    );
}

unsafe fn read_thread_func(mut global_conf: ShardPtr<GlobalConf>) {
    set_cpu_affinity();
    let global_conf = global_conf.as_mut();
    let checker = TestObj::new(&mut global_conf.cnt);
    for _ in 0..global_conf.read_loops {
        let mut handle = 0u64;
        let ret = global_conf.h.acquire(&mut handle);
        assert_eq!(ret, Status::Success);
        let v = util::atomic_load_raw_ptr(&global_conf.v);
        assert!(*v == checker);
        global_conf.h.release(handle);
    }
}

unsafe fn write_thread_func(mut global_conf: ShardPtr<GlobalConf>) {
    set_cpu_affinity();
    let global_conf = global_conf.as_mut();
    for _ in 0..global_conf.write_loops {
        let v = Box::into_raw(Box::new(TestObj::new(&mut global_conf.cnt)));
        let mut curr = util::atomic_load_raw_ptr(&global_conf.v);
        let mut old = curr;
        while !{
            let (tmp, b) = util::atomic_cxchg_raw_ptr(&mut global_conf.v, old, v);
            curr = tmp;
            b
        } {
            old = curr;
        }
        global_conf.h.add_node(old);
    }
}

unsafe fn debug_thread_func(global_conf: ShardPtr<GlobalConf>) {
    while !global_conf.as_ref().stop() {
        println!(
            "hazard_waiting_count={}",
            global_conf.as_ref().h.get_hazard_waiting_count()
        );
        thread::sleep(time::Duration::from_millis(1000));
    }
}

struct ShardPtr<T>(pub *mut T);

unsafe impl<T> Send for ShardPtr<T> {}

unsafe impl<T> Sync for ShardPtr<T> {}

impl<T> ShardPtr<T> {
    fn new(data: *mut T) -> Self {
        ShardPtr(data)
    }

    fn as_ref(&self) -> &T {
        unsafe { &*self.0 }
    }

    fn as_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0 }
    }
}

impl<T> Copy for ShardPtr<T> {}

impl<T> Clone for ShardPtr<T> {
    fn clone(&self) -> Self {
        ShardPtr(self.0)
    }
}

impl<T> Deref for ShardPtr<T> {
    type Target = *mut T;

    fn deref(&self) -> &<Self as Deref>::Target {
        &self.0
    }
}

impl<T> DerefMut for ShardPtr<T> {
    fn deref_mut(&mut self) -> &mut <Self as Deref>::Target {
        &mut self.0
    }
}

#[test]
fn test_multi_thread() {
    env_logger::init();

    let cpu_count = core_affinity::get_core_ids().unwrap().len() as i64;

    let read_count = (cpu_count + 1) / 2;
    let write_count = (cpu_count + 1) / 2;

    println!("read thread {}, write thread {}", read_count, write_count);

    let memory = 1024_i64 * 1024 * 1024; // 1G
    let cnt = memory / mem::size_of::<TestObj>() as i64 / write_count;

    let mut global_conf = unsafe { mem::zeroed::<GlobalConf>() };
    global_conf.stop = 0;
    global_conf.cnt = 0;
    global_conf.read_loops = cnt;
    global_conf.write_loops = cnt;
    global_conf.v = Box::into_raw(Box::new(TestObj::new(&mut global_conf.cnt)));
    global_conf.h = unsafe { HazardEpoch::default_new_in_stack() };
    let global_conf_ptr = ShardPtr::new(&mut global_conf as *mut _);

    println!(
        "read loops {}, write loops {}",
        global_conf.read_loops, global_conf.write_loops
    );

    let mut rpd = vec![];
    let mut wpd = vec![];
    let dpd = thread::spawn(move || unsafe { debug_thread_func(global_conf_ptr) });
    for _ in 0..read_count {
        rpd.push(thread::spawn(move || unsafe {
            read_thread_func(global_conf_ptr)
        }));
    }
    for _ in 0..write_count {
        wpd.push(thread::spawn(move || unsafe {
            write_thread_func(global_conf_ptr)
        }));
    }

    for t in rpd {
        t.join().unwrap();
    }

    println!("read threads joined");

    for t in wpd {
        t.join().unwrap();
    }

    println!("write threads joined");

    unsafe {
        global_conf.set_stop(true);
    }

    dpd.join().unwrap();

    unsafe {
        ptr::drop_in_place(global_conf.v);
    }

    unsafe {
        global_conf.h.retire();
    }
    assert_eq!(0, global_conf.cnt);
}

#[test]
fn test_base() {
    unsafe {
        let mut he = Box::new(HazardEpoch::default_new_in_stack());
        let mut cnt = 0i64;
        let mut handle = 0u64;
        let ret = he.acquire(&mut handle);
        assert_eq!(ret, Status::Success);
        for i in 0..64i64 {
            let tmp = Box::new(TestObj::new(&mut cnt));
            let ret = he.add_node(Box::into_raw(tmp));
            assert_eq!(Status::Success, ret);
            assert_eq!(i + 1, cnt);
        }
        he.retire();
        assert_eq!(cnt, 64);
        he.release(handle);
        he.retire();
        assert_eq!(cnt, 0);

        for i in 0..32i64 {
            assert_eq!(
                he.add_node(Box::into_raw(Box::new(TestObj::new(&mut cnt)))),
                Status::Success
            );
            assert_eq!(cnt, i + 1);
        }

        assert_eq!(he.acquire(&mut handle), Status::Success);
        for i in 32..64i64 {
            let tmp = Box::new(TestObj::new(&mut cnt));
            let ret = he.add_node(Box::into_raw(tmp));
            assert_eq!(Status::Success, ret);
            assert_eq!(i + 1, cnt);
        }

        he.retire();
        assert_eq!(32, cnt);
        he.release(handle);
        he.retire();
        assert_eq!(cnt, 0);

        for _ in 0..2i64 {
            assert_eq!(he.acquire(&mut handle), Status::Success);
            assert_eq!(he.acquire(&mut handle), Status::Busy);
            he.release(handle);
        }
    }
}
