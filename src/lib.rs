#![feature(core_intrinsics)]
#![feature(raw)]
#![allow(dead_code)]
#![feature(allocator_api)]

pub mod hazard_pointer;
pub mod util;
pub mod error;
pub mod hazard_epoch;
mod spin_lock;
pub mod lockfree_queue;
pub mod lockfree_stack;

#[macro_use]
extern crate log;
