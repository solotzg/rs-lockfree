//! Lock-Free lib based on practical `Hazard Pointers` algorithm for Rust
//!
//! [`Hazard Pointers`](http://www.cs.otago.ac.nz/cosc440/readings/hazard-pointers.pdf) algorithm
//! firstly saves the pointer of shared object to local thread, and then accessed it, and removes it
//! after accessing is over. An object can be released only when there is no thread contains its
//! reference, which solve the [`ABA problem`](https://en.wikipedia.org/wiki/ABA_problem).
//!
//! Theoretically, `Hazard Pointers` resolve the performance bottleneck problem causing by atomic
//! reference counting, but also has some Inadequacies: It costs lot of time to traverse global
//! array when reclaiming memory; Traversing global array doesn't guarantee atomicity; Each shared
//! object should maintain relationships with all threads, which increases the cost of usage.
//!
//! We provide `HazardEpoch`, a practical implementation of `Hazard Pointers`, which make further
//! improvement and provide an easier way for usage.
//! `LockFreeQueue` and `LockFreeStack`, implemented based on `HazardEpoch`, contain a few simple
//! methods like `push`, `pop`.
//!
#![feature(core_intrinsics)]
#![feature(raw)]
#![allow(dead_code)]

mod hazard_pointer;
pub mod util;
pub mod error;
pub mod hazard_epoch;
pub mod spin_lock;
pub mod spin_rwlock;
pub mod lockfree_queue;
pub mod lockfree_stack;

#[macro_use]
extern crate log;

#[macro_use]
extern crate cfg_if;
