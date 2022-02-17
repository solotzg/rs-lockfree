# rs-lockfree


[![Build Status](https://travis-ci.org/solotzg/rs-lockfree.svg?branch=master)](https://travis-ci.org/solotzg/rs-lockfree)
[![Crates.io](https://img.shields.io/crates/v/rs_lockfree.svg)](https://crates.io/crates/rs_lockfree)
* `Concurrently R/W Shared Object` is one of the most frequent operations in high performance concurrent program. 
There are several ways to implement it, such as `R/W Lock`, `Reference Counting`, `Hazard Pointers`, `Epoch Based Reclamation`, 
`Quiescent State Based Reclamation`.
* When faced with lock contention, current thread will usually be suspended and wait for trigger to wake up. 
Therefore, to improve performance, lock-free structure is our first choice if the cost of retrying operation is lower 
than context switch. Obviously, atomic operations, such as CAS, are essential in lock-free programming. 
* Reference counting has 2 deficiencies: 
    - Each reading needs to modify the global reference count. Atomic operations on the same object in high concurrency 
situation may become a performance bottleneck. 
    - Managing ref-object will bring additional maintenance costs and increase implementation complexity.
* [`Hazard Pointers`](http://www.cs.otago.ac.nz/cosc440/readings/hazard-pointers.pdf) algorithm firstly saves the 
pointer of shared object to local thread, and then accessed it, and removes it after accessing is over. An object can 
be released only when there is no thread contains its reference, which solve the [`ABA problem`](https://en.wikipedia.org/wiki/ABA_problem). 
* Hazard Pointers resolve the performance bottleneck problem caused by atomic reference counting, but also has some 
inadequacies:
    - Each shared object should maintain relationships with threads, which increases the cost of usage.
    - It costs lot of time to traverse global array when reclaiming memory.
    - Traversing global array can't guarantee atomicity.
* OceanBase provides a practical way to use Hazard Pointers to implement a lock-free structure, which inspires me a lot. 
    - GV(global Incremental version) is needed to identify the shared object to be reclaimed. 
    - Before accessing a shared object, save the GV to local thread and name it as TV
    - When reclaiming a shared object, firstly, save GV to it as OV and atomic_add(GV, 1); secondly, traverse all 
    threads and find the minimum TV as RV; finally, reclaim this shared object if OV < RV.
    - Mechanism like delayed reclaim is needed because traversing array will cost much time.

# Usage
* So far, this lib only supports `x86_64` arch because there are few scenes other than high-performance server program need
lock-free solution.
* Because of [`False sharing`](https://en.wikipedia.org/wiki/False_sharing), a part of the member variables, might be 
frequently modified by different threads, are aligned to 64 bytes. And this may lead to stack overflow while initializing.
So, 3 features are provided in `Cargo.toml`: max_thread_count_16(default), max_thread_count_256, 
max_thread_count_4096(need to manually change minimum stack size or set RUST_MIN_STACK to 6000000).
* Most allocators chose 128K(default setting) as the threshold to decide whether to allocate memory by `mmap`. In 
order to improve performance, it's better to allocate [`HazardEpoch`](src/hazard_epoch.rs), [`LockFreeQueue`](src/lockfree_queue.rs) or [`LockFreeStack`](src/lockfree_stack.rs) in stack.

* Examples
    - `example_hazard_epoch` show the scene that multiple producers and multiple reader deal with one config. Run command:
        ```
        RUST_LOG=INFO cargo run --release --example example_hazard_epoch
        ```
    - `example_lockfree_queue` show the scene with multiple producers and multiple consumers. Run command:
        ```
        RUST_LOG=INFO cargo run --release --example example_lockfree_queue
        ```
    - `example_lockfree_stack` show the scene with multiple producers and multiple consumers. Run command:
        ```
        RUST_LOG=INFO cargo run --release --example example_lockfree_stack
        ```

# Change Logs
* version `0.1.1`
  - Remove the use of `allocator_api`, because rust-nightly changes this feature too damn frequently.
