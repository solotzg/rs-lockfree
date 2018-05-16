#![feature(core_intrinsics)]

extern crate core_affinity;
extern crate rs_lockfree;
#[macro_use]
extern crate log;
extern crate env_logger;

use rs_lockfree::lockfree_queue;
use rs_lockfree::util;
use std::ops::Deref;
use std::ops::DerefMut;
use std::mem;
use std::thread;
use std::intrinsics;
use std::time;
use std::time::SystemTime;

#[repr(align(16))]
#[derive(Default)]
struct QueueValue {
    value: i64,
}

struct GlobalControl {
    queue: lockfree_queue::LockFreeQueue<QueueValue>,
    loop_cnt: i64,
    producer_cnt: i64,
    produced: i64,
    consumed: i64,
    tol_val: i64,
}

struct ShardPtr<T>(pub *mut T);

unsafe impl<T> Send for ShardPtr<T> {}

unsafe impl<T> Sync for ShardPtr<T> {}

impl<T> ShardPtr<T> {
    fn new(data: *mut T) -> Self {
        ShardPtr(data)
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

fn set_cpu_affinity() {
    let cpus = core_affinity::get_core_ids().unwrap();
    core_affinity::set_for_current(cpus[util::get_thread_id() as usize % cpus.len()]);
    info!(
        "set_cpu_affinity {} {}",
        util::get_thread_id(),
        util::get_thread_id() as usize % cpus.len()
    );
}

unsafe fn consumer_thread(mut global_control: ShardPtr<GlobalControl>) {
    set_cpu_affinity();
    let global_control = global_control.as_mut();
    let mut ret = false;
    let mut tol = 0;
    let mut tol_val = 0;
    loop {
        if let Some(v) = global_control.queue.pop() {
            let val = v.value;
            tol_val += val;
            tol += 1;
            if tol % 1024 == 0 {
                intrinsics::atomic_xadd(&mut global_control.consumed, tol);
                intrinsics::atomic_xadd(&mut global_control.tol_val, tol_val);
                tol = 0;
                tol_val = 0;
            }
            ret = false;
        } else {
            if intrinsics::atomic_load(&global_control.producer_cnt) == 0 {
                if ret {
                    break;
                } else {
                    ret = true;
                }
            }
        }
    }
    intrinsics::atomic_xadd(&mut global_control.consumed, tol);
    intrinsics::atomic_xadd(&mut global_control.tol_val, tol_val);
}

unsafe fn producer_thread(mut global_control: ShardPtr<GlobalControl>) {
    set_cpu_affinity();
    let global_control = global_control.as_mut();
    let mut tol = 0;
    let loop_cnt = global_control.loop_cnt;
    for i in 0..loop_cnt {
        global_control.queue.push(QueueValue { value: i });
        tol += 1;
        if i % 1024 == 0 {
            intrinsics::atomic_xadd(&mut global_control.produced, tol);
            tol = 0;
        }
    }
    intrinsics::atomic_xadd(&mut global_control.produced, tol);
    util::sync_fetch_and_add(&mut global_control.producer_cnt, -1);
}

unsafe fn debug_thread(mut global_control: ShardPtr<GlobalControl>) {
    let global_control = global_control.as_mut();
    while intrinsics::atomic_load(&global_control.producer_cnt) != 0 {
        info!(
            "debug_thread produced {} consumed {}",
            intrinsics::atomic_load(&global_control.produced),
            intrinsics::atomic_load(&global_control.consumed)
        );
        thread::sleep(time::Duration::from_millis(1000));
    }
}

fn main() {
    let start = SystemTime::now();
    thread::spawn(|| {
        test_multi_threads();
    }).join()
        .unwrap();
    let end = SystemTime::now();
    let cost = {
        let t = end.duration_since(start).unwrap();
        t.subsec_millis() as u64 + t.as_secs() * 1000
    };
    println!("time cost {} ms", cost);
}

fn test_multi_threads() {
    env_logger::init();

    let cpu_count = core_affinity::get_core_ids().unwrap().len() as i64;

    let producer_count = (cpu_count + 1) / 2;
    let consumer_count = cpu_count - producer_count;

    info!(
        "producer_count {} consumer_count {}",
        producer_count, consumer_count
    );

    let memory = 2048_i64 * 1024 * 1024; // 2G
    let cnt = memory / mem::size_of::<QueueValue>() as i64 / producer_count;

    info!("loop_cnt {}, total need {}", cnt, cnt * producer_count);

    let mut global_control = unsafe { mem::zeroed::<GlobalControl>() };

    global_control.loop_cnt = cnt;
    global_control.queue = unsafe { lockfree_queue::LockFreeQueue::default_new_in_stack() };
    global_control.producer_cnt = producer_count;

    let global_control_ptr = ShardPtr::new(&mut global_control as *mut _);

    let mut producer_threads = vec![];
    let mut consumer_threads = vec![];

    let watch_thread = thread::spawn(move || unsafe {
        debug_thread(global_control_ptr);
    });

    for _ in 0..producer_count {
        producer_threads.push(thread::spawn(move || unsafe {
            producer_thread(global_control_ptr);
        }));
    }

    for _ in 0..consumer_count {
        consumer_threads.push(thread::spawn(move || unsafe {
            consumer_thread(global_control_ptr);
        }));
    }

    for t in producer_threads {
        t.join().unwrap();
    }

    info!("producer_threads joined");

    for t in consumer_threads {
        t.join().unwrap();
    }

    info!("consumer_threads joined");

    watch_thread.join().unwrap();

    let (produced, consumed) = unsafe {
        (
            intrinsics::atomic_load(&global_control.produced),
            intrinsics::atomic_load(&global_control.consumed),
        )
    };
    info!("debug_thread produced {} consumed {}", produced, consumed);
    assert_eq!(
        global_control.tol_val,
        producer_count * (global_control.loop_cnt - 1) * global_control.loop_cnt / 2
    );
    assert_eq!(produced, consumed);
}
