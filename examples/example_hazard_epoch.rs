#![feature(core_intrinsics)]

extern crate core_affinity;
extern crate env_logger;
extern crate rs_lockfree;
#[macro_use]
extern crate log;

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
    written: i64,
    read: i64,
}

impl GlobalConf {
    unsafe fn set_stop(&mut self, stop: bool) {
        intrinsics::atomic_store(&mut self.stop, stop as u8);
    }

    unsafe fn add_written_cnt(&mut self, add: i64) {
        intrinsics::atomic_xadd(&mut self.written, add);
    }

    unsafe fn add_read_cnt(&mut self, add: i64) {
        intrinsics::atomic_xadd(&mut self.read, add);
    }

    unsafe fn stop(&self) -> bool {
        intrinsics::atomic_load(&self.stop) != 0
    }
}

fn set_cpu_affinity() {
    let cpus = core_affinity::get_core_ids().unwrap();
    core_affinity::set_for_current(cpus[util::get_thread_id() as usize % cpus.len()]);
    info!(
        "set_cpu_affinity {} {}",
        util::get_thread_id(),
        util::get_thread_id() as usize % cpus.len()
    );
}

unsafe fn reader_thread_func(mut global_conf: ShardPtr<GlobalConf>) {
    set_cpu_affinity();
    let global_conf = global_conf.as_mut();
    let mut tol = 0;
    for _ in 0..global_conf.read_loops {
        let mut handle = 0u64;
        let ret = global_conf.h.acquire(&mut handle);
        assert_eq!(ret, Status::Success);
        let v = util::atomic_load_raw_ptr(&global_conf.v);
        assert!((*v).data.is_some());
        global_conf.h.release(handle);
        tol += 1;
        if tol % 512 == 0 {
            global_conf.add_read_cnt(tol);
            tol = 0;
        }
    }
    global_conf.add_read_cnt(tol);
}

unsafe fn producer_thread_func(mut global_conf: ShardPtr<GlobalConf>) {
    set_cpu_affinity();
    let global_conf = global_conf.as_mut();
    let mut tol = 0;
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
        tol += 1;
        if tol % 512 == 0 {
            global_conf.add_written_cnt(tol);
            tol = 0;
        }
        global_conf.h.add_node(old);
    }
    global_conf.add_written_cnt(tol);
}

unsafe fn debug_thread_func(global_conf: ShardPtr<GlobalConf>) {
    let global_conf = global_conf.as_ref();
    while !global_conf.stop() {
        info!(
            "waiting_count {} written {} read {}",
            global_conf.h.get_hazard_waiting_count(),
            global_conf.written,
            global_conf.read,
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

fn main() {
    thread::spawn(|| run()).join().unwrap();
}

fn run() {
    env_logger::init();

    let cpu_count = core_affinity::get_core_ids().unwrap().len() as i64;

    let read_count = (cpu_count + 1) / 2;
    let write_count = (cpu_count + 1) / 2;

    info!("read thread {}, write thread {}", read_count, write_count);

    let memory = 2048_i64 * 1024 * 1024; // 2G
    let cnt = memory / mem::size_of::<TestObj>() as i64 / write_count;

    let mut global_conf = unsafe { mem::zeroed::<GlobalConf>() };
    global_conf.read_loops = cnt;
    global_conf.write_loops = cnt;
    global_conf.v = Box::into_raw(Box::new(TestObj::new(&mut global_conf.cnt)));
    global_conf.h = unsafe { HazardEpoch::default_new_in_stack() };
    let global_conf_ptr = ShardPtr::new(&mut global_conf as *mut _);

    info!(
        "read loops {}, write loops {}",
        global_conf.read_loops, global_conf.write_loops
    );

    let mut rpd = vec![];
    let mut wpd = vec![];
    let dpd = thread::spawn(move || unsafe { debug_thread_func(global_conf_ptr) });
    for _ in 0..read_count {
        rpd.push(thread::spawn(move || unsafe {
            reader_thread_func(global_conf_ptr)
        }));
    }
    for _ in 0..write_count {
        wpd.push(thread::spawn(move || unsafe {
            producer_thread_func(global_conf_ptr)
        }));
    }

    for t in rpd {
        t.join().unwrap();
    }

    info!("read threads joined");

    for t in wpd {
        t.join().unwrap();
    }

    info!("write threads joined");

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

    info!(
        "waiting_count {} written {} read {}",
        unsafe { global_conf.h.get_hazard_waiting_count() },
        global_conf.written,
        global_conf.read,
    );
    assert_eq!(0, global_conf.cnt);
    assert_eq!(global_conf.read, global_conf.written);
}
