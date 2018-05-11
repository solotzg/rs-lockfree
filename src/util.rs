extern crate time;

use std::ops::{Deref, DerefMut};
use std::sync::atomic;

/// Wrap struct into WrappedAlign64Type to make it 64bytes aligned.
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

/// Return current unix timestamp(microsecond).
pub fn get_cur_microseconds_time() -> i64 {
    let timespec = time::get_time();
    timespec.sec * 1_000_000 + timespec.nsec as i64 / 1_000
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod atomic_x86 {
    use std::ops::Add;
    use std::intrinsics;
    use std::mem;
    use std::cell::Cell;

    /// Auto increase global thread id.
    pub static mut GLOBAL_THREAD_ID: Cell<i64> = Cell::new(-1);

    /// Return an unique ID for current thread.
    pub fn get_thread_id() -> i64 {
        thread_local!(static THREAD_ID: Cell<i64> = Cell::new(-1););
        THREAD_ID.with(|tid| {
            if -1 == tid.get() {
                tid.set(unsafe { sync_fetch_and_add(GLOBAL_THREAD_ID.get_mut(), 1) });
            }
            tid.get()
        })
    }

    /// Like __sync_add_and_fetch in C.
    pub unsafe fn sync_add_and_fetch<T>(dst: *mut T, src: T) -> T
    where
        T: Add<Output = T> + Copy,
    {
        intrinsics::atomic_xadd::<T>(dst, src) + src
    }

    /// Like __sync_fetch_and_add in C.
    pub unsafe fn sync_fetch_and_add<T>(dst: *mut T, src: T) -> T {
        intrinsics::atomic_xadd::<T>(dst, src)
    }

    /// Atomic load raw pointer.
    pub unsafe fn atomic_load_raw_ptr<T>(ptr: *const *mut T) -> *mut T {
        intrinsics::atomic_load(ptr as *const usize) as *mut T
    }

    /// Atomic CAS raw pointer.
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
}

pub use self::atomic_x86::*;

/// Yield current thread.
#[inline]
pub fn pause() {
    atomic::spin_loop_hint();
}
