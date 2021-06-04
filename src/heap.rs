use std::cell::UnsafeCell;

use super::bucket::*;

#[repr(C)]
struct Heap {
    small_buckets: [UnsafeCell<Bucket>; SMALL_BUCKETS + LARGE_BUCKETS],
}
