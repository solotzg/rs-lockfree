#![feature(core_intrinsics)]

extern crate core_affinity;
extern crate rs_lockfree;

use rs_lockfree::lockfree_queue;
use rs_lockfree::util;
use std::ops::Deref;
use std::ops::DerefMut;
use std::mem;
use std::thread;
use std::intrinsics;
use std::time;

#[derive(Default)]
struct QueueValue {
    a: i64,
    b: i64,
    sum: i64,
}

struct GlobalConf {
    queue: lockfree_queue::LockFreeQueue<QueueValue>,
    loop_cnt: i64,
    producer_cnt: i64,
    produced: i64,
    consumed: i64,
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

unsafe fn consumer_thread(mut global_conf: ShardPtr<GlobalConf>) {
    set_cpu_affinity();
    let global_conf = global_conf.as_mut();
    let mut ret = false;
    let mut tol = 0;
    loop {
        if let Some(v) = global_conf.queue.pop() {
            assert_eq!(v.a + v.b, v.sum);
            tol += 1;
            if tol % 512 == 0 {
                intrinsics::atomic_xadd(&mut global_conf.consumed, tol);
                tol = 0;
            }
            ret = false;
        } else {
            if intrinsics::atomic_load(&global_conf.producer_cnt) == 0 {
                if ret {
                    break;
                } else {
                    ret = true;
                }
            }
        }
    }
    intrinsics::atomic_xadd(&mut global_conf.consumed, tol);
}

unsafe fn producer_thread(mut global_conf: ShardPtr<GlobalConf>) {
    set_cpu_affinity();
    let global_conf = global_conf.as_mut();
    let sum_base = util::get_thread_id() * global_conf.loop_cnt;
    let mut tol = 0;
    for i in 0..global_conf.loop_cnt {
        global_conf.queue.push(QueueValue {
            a: i,
            b: 2 * i + sum_base,
            sum: sum_base + i * 3,
        });
        tol += 1;
        if i % 512 == 0 {
            intrinsics::atomic_xadd(&mut global_conf.produced, tol);
            tol = 0;
        }
    }
    intrinsics::atomic_xadd(&mut global_conf.produced, tol);
    util::sync_fetch_and_add(&mut global_conf.producer_cnt, -1);
}

unsafe fn debug_thread(mut global_conf: ShardPtr<GlobalConf>) {
    let global_conf = global_conf.as_mut();
    while intrinsics::atomic_load(&global_conf.producer_cnt) != 0 {
        println!(
            "debug_thread produced {} consumed {}",
            intrinsics::atomic_load(&global_conf.produced),
            intrinsics::atomic_load(&global_conf.consumed)
        );
        thread::sleep(time::Duration::from_millis(1000));
    }
}

#[test]
fn test_multi_threads() {
    let cpu_count = core_affinity::get_core_ids().unwrap().len() as i64;

    let producer_count = (cpu_count + 1) / 2;
    let consumer_count = cpu_count - producer_count;

    println!(
        "producer_count {} consumer_count {}",
        producer_count, consumer_count
    );

    let memory = 256_i64 * 1024 * 1024; // 256M
    let cnt = memory / mem::size_of::<QueueValue>() as i64 / producer_count;

    println!("loop_cnt {}, total need {}", cnt, cnt * producer_count);

    let mut global_conf = unsafe { mem::zeroed::<GlobalConf>() };

    global_conf.loop_cnt = cnt;
    global_conf.queue = lockfree_queue::LockFreeQueue::new();
    global_conf.producer_cnt = producer_count;

    let global_conf_ptr = ShardPtr::new(&mut global_conf as *mut _);

    let mut producer_threads = vec![];
    let mut consumer_threads = vec![];

    let watch_thread = thread::spawn(move || unsafe {
        debug_thread(global_conf_ptr);
    });

    for _ in 0..producer_count {
        producer_threads.push(thread::spawn(move || unsafe {
            producer_thread(global_conf_ptr);
        }));
    }

    for _ in 0..consumer_count {
        consumer_threads.push(thread::spawn(move || unsafe {
            consumer_thread(global_conf_ptr);
        }));
    }

    for t in producer_threads {
        t.join().unwrap();
    }

    println!("producer_threads joined");

    for t in consumer_threads {
        t.join().unwrap();
    }

    println!("consumer_threads joined");

    watch_thread.join().unwrap();

    let (produced, consumed) = unsafe {
        (
            intrinsics::atomic_load(&global_conf.produced),
            intrinsics::atomic_load(&global_conf.consumed),
        )
    };
    println!("debug_thread produced {} consumed {}", produced, consumed);

    assert_eq!(produced, consumed);
}
