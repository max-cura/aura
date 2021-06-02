use std::cell::UnsafeCell;

use super::bucket::*;

#[repr(C)]
struct Heap {
    small_buckets: [UnsafeCell<Bucket>; SMALL_BUCKETS],
    large_buckets: [UnsafeCell<Bucket>; LARGE_BUCKETS],
}
