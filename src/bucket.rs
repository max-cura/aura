use std::cell::UnsafeCell;
use std::default::Default;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::{intrinsics, mem, ptr};

use parking_lot::RawMutex;

use super::block::BlockHeader;
use super::free_list::{AtomicPushFreeList, FreeListPush};
use crate::constants::KB;
use crate::util::extrinsic_bsr;

#[repr(C)]
pub struct Bucket {
    // size is implicit
    active: AtomicPtr<BlockHeader>,
    maybe_free_list: AtomicPtr<BlockHeader>,
    maybe_mesh_list: AtomicPtr<BlockHeader>,
    count: AtomicUsize,
}

// Primary path
impl Bucket {
    pub fn alloc(&mut self) -> *mut u8 {
        let maybe_active = self.active.load(Ordering::SeqCst);
        if maybe_active.is_null() {
            // TODO generic path
        }
        let maybe_active = unsafe { &mut *maybe_active };
        let maybe_object = maybe_active.alloc();
        if !maybe_object.is_null() {
            return maybe_object
        }
        // TODO generic path
        maybe_object
    }

    fn source_block(&mut self) -> *mut BlockHeader { ptr::null_mut() }
}

// Infrastructure
impl Bucket {
    pub fn new() -> Bucket { Bucket::default() }

    /// Invariant(never bucket'maybe_free [@ block block])
    pub fn maybe_free(&mut self, block_header: *mut BlockHeader) {
        let mut curr = self.maybe_free_list.load(Ordering::SeqCst);

        loop {
            unsafe { &mut *block_header }._set_maybe_next_free(curr);
            match self.maybe_free_list.compare_exchange_weak(
                curr,
                block_header,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => curr = actual,
            }
        }
    }
}

// Other traits
impl Default for Bucket {
    fn default() -> Self {
        Bucket {
            active: AtomicPtr::new(ptr::null_mut()),
            maybe_free_list: AtomicPtr::new(ptr::null_mut()),
            maybe_mesh_list: AtomicPtr::new(ptr::null_mut()),
            count: AtomicUsize::new(0),
        }
    }
}

pub const fn bucket_select_tiny(size: usize) -> usize { 0 }

pub const fn bucket_select_small(size: usize) -> usize { 0 }

pub const fn bucket_select_large(size: usize) -> usize { 0 }

pub const fn bucket_to_size(bucket: usize) -> usize { 0 }

pub const fn semi_logarithmic_interval(lower: usize, upper: usize) -> usize { 0 }

#[cfg(test)]
mod tests {
    use super::{
        bucket_select_large, bucket_select_small, bucket_select_tiny, bucket_to_size,
        semi_logarithmic_interval, LARGE_BUCKETS, LARGE_OBJECT_BOUNDARY, NONTINY_SMALL_BUCKETS,
        SMALL_BUCKETS, SMALL_OBJECT_BOUNDARY, TINY_OBJECT_BOUNDARY, TINY_SMALL_BUCKETS,
    };
    #[test]
    fn bucket_tiny() {
        for size in 1..=16 {
            assert_eq!(bucket_select_tiny(size), 1);
        }
        assert_eq!(bucket_to_size(0), 16);
        for bucket in 1..TINY_SMALL_BUCKETS {
            for i_size in 1..=8 {
                assert_eq!(bucket_select_tiny(16 + (bucket - 1) * 8 + 8), bucket);
            }
            assert_eq!(bucket_to_size(bucket), 16 + (bucket - 1) * 8 + 8);
        }
    }
    #[test]
    fn bucket_small() {
        unimplemented!();
    }
    #[test]
    fn bucket_large() {
        unimplemented!();
    }
}

pub const TINY_OBJECT_BOUNDARY: usize = 512;
const TINY_BUCKET_STEP: usize = 8;
pub const SMALL_OBJECT_BOUNDARY: usize = 8 * KB;
pub const LARGE_OBJECT_BOUNDARY: usize = 512 * KB;

const TINY_SMALL_BUCKETS: usize = TINY_BUCKET_STEP / 8 - 1;
pub const NONTINY_SMALL_BUCKETS: usize =
    semi_logarithmic_interval(TINY_OBJECT_BOUNDARY, SMALL_OBJECT_BOUNDARY);

pub const SMALL_BUCKETS: usize = TINY_SMALL_BUCKETS + NONTINY_SMALL_BUCKETS;
pub const LARGE_BUCKETS: usize =
    semi_logarithmic_interval(SMALL_OBJECT_BOUNDARY, LARGE_OBJECT_BOUNDARY);
