extern crate time;

use std::cell::Cell;

use std::intrinsics;
use std::ops::{Add, Deref, DerefMut};
use std::sync::atomic;
use std::mem;

pub static mut GLOBAL_THREAD_ID: i64 = 0;

pub unsafe fn get_thread_id() -> i64 {
    thread_local!(static THREAD_ID: Cell<i64> = Cell::new(-1););
    THREAD_ID.with(|tid| {
        if -1 == tid.get() {
            tid.set(sync_fetch_and_add(&mut GLOBAL_THREAD_ID as *mut i64, 1));
        }
        tid.get()
    })
}

#[repr(align(64))]
pub struct WrappedAlign64Type<T>(pub T);

impl<T> Default for WrappedAlign64Type<T>
where
    T: Default,
{
    fn default() -> Self {
        WrappedAlign64Type(T::default())
    }
}

impl<T> Deref for WrappedAlign64Type<T> {
    type Target = T;

    fn deref(&self) -> &<Self as Deref>::Target {
        &self.0
    }
}

impl<T> DerefMut for WrappedAlign64Type<T> {
    fn deref_mut(&mut self) -> &mut <Self as Deref>::Target {
        &mut self.0
    }
}

pub fn get_cur_microseconds_time() -> i64 {
    let timespec = time::get_time();
    timespec.sec * 1_000_000 + timespec.nsec as i64 / 1_000
}

pub unsafe fn sync_add_and_fetch<T>(dst: *mut T, src: T) -> T
where
    T: Add<Output = T> + Copy,
{
    intrinsics::atomic_xadd::<T>(dst, src) + src
}

pub unsafe fn sync_fetch_and_add<T>(dst: *mut T, src: T) -> T {
    intrinsics::atomic_xadd::<T>(dst, src)
}

pub unsafe fn atomic_load_raw_ptr<T>(ptr: *const *const T) -> *mut T {
    intrinsics::atomic_load(ptr as *const usize) as *mut T
}

pub unsafe fn atomic_cxchg_raw_ptr<T>(
    ptr: *mut *mut T,
    old: *mut T,
    src: *mut T,
) -> (*mut T, bool) {
    mem::transmute(intrinsics::atomic_cxchg(
        ptr as *mut usize,
        old as usize,
        src as usize,
    ))
}

#[inline]
pub fn pause() {
    atomic::spin_loop_hint();
}
