use std::cell::UnsafeCell;

use super::bucket::*;

#[repr(C)]
struct Heap<'a> {
    small_buckets: [UnsafeCell<Bucket<'a>>; SMALL_BUCKETS],
    large_buckets: [UnsafeCell<Bucket<'a>>; LARGE_BUCKETS],
}
