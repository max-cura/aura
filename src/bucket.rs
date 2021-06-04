use std::cell::UnsafeCell;
use std::default::Default;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::{intrinsics, mem, ptr};

use parking_lot::RawMutex;

use super::block::BlockHeader;
use super::free_list::{AtomicPushFreeList, FreeListPush};
use super::{bucket, top_level};
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

    fn source_block(&mut self) -> *mut BlockHeader {
        // 1. clean up free list

        let mut first = None;
        let mut free_list = self.maybe_free_list.swap(ptr::null_mut(), Ordering::SeqCst);
        let top_level = top_level::get();
        while !free_list.is_null() {
            let free_list_ref = unsafe { &mut *free_list };
            // Once it's in OUR free list, that means that it belongs to this
            // thread, and it was empty at some point
            // in order to become non-free, it must be allocated from
            // in order to be allocated from, it must become active
            // in order to become active, this function must have been run
            // therefore it is empty. QED.
            assert_eq!(free_list_ref.allocated(), 0);
            match first {
                None => first = Some(free_list),
                Some(_) => {
                    top_level.receive(
                        bucket::bucket_select(free_list_ref._object_size()),
                        unsafe {
                            free_list_ref.get_segment().block_header(free_list_ref._segment_idx())
                        },
                    );
                },
            };

            free_list = free_list_ref._maybe_next_free();
        }

        if let Some(bh) = first {
            return bh
        }

        ptr::null_mut()
    }
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

/// size to (tiny) bucket ind
const fn bucket_select_tiny(size: usize) -> usize {
    if size < 16 {
        0
    } else {
        (size - 16) / TINY_BUCKET_STEP
    }
}

const fn bucket_select_semilogarithmic<const BOUNDARY: usize>(size: usize) -> usize
where
    [(); extrinsic_bsr(BOUNDARY)]: Sized,
{
    let size_bsr = extrinsic_bsr(size);
    let mult = size_bsr - const { extrinsic_bsr(BOUNDARY) };
    let idx = size_bsr - 3;
    let det = (size >> idx) & 3;
    4 * mult + det
}
pub const fn bucket_to_size(bucket: usize) -> usize {
    if bucket < TINY_SMALL_BUCKETS {
        bucket * 8 + 16
    } else {
        let bucket = bucket - TINY_SMALL_BUCKETS;
        let size_bsr = bucket / 4 + const { extrinsic_bsr(TINY_OBJECT_BOUNDARY) };
        let det = bucket % 4;
        let det = det | 4;
        let idx = size_bsr - 3;
        let size = det << idx;
        size
    }
}

pub const fn bucket_select(size: usize) -> usize {
    if size < TINY_OBJECT_BOUNDARY {
        bucket_select_tiny(size)
    } else {
        bucket_select_semilogarithmic::<TINY_OBJECT_BOUNDARY>(size) + TINY_SMALL_BUCKETS
    }
}

const fn semi_logarithmic_interval(lower_boundary: usize, upper_boundary: usize) -> usize {
    let interval = extrinsic_bsr(upper_boundary) - extrinsic_bsr(lower_boundary);
    8 + (interval - 3) * 4
}

#[cfg(test)]
mod tests {
    use super::{
        bucket_select, bucket_to_size, semi_logarithmic_interval, LARGE_BUCKETS,
        LARGE_OBJECT_BOUNDARY, NONTINY_SMALL_BUCKETS, SMALL_BUCKETS, SMALL_OBJECT_BOUNDARY,
        TINY_OBJECT_BOUNDARY, TINY_SMALL_BUCKETS,
    };
    #[test]
    fn bucket_tiny() {
        for size in 1..16 {
            assert_eq!(bucket_select(size), 0);
        }
        assert_eq!(bucket_to_size(0), 16);
        for bucket in 0..(TINY_SMALL_BUCKETS - 1) {
            for i_size in 0..8 {
                assert_eq!(bucket_select(16 + bucket * 8 + i_size), bucket);
            }
            assert_eq!(bucket_to_size(bucket), 16 + bucket * 8);
        }
    }
    #[test]
    fn bucket_semilogarithmic() {
        assert_eq!(bucket_to_size(TINY_SMALL_BUCKETS), TINY_OBJECT_BOUNDARY);
        for size in TINY_OBJECT_BOUNDARY..LARGE_OBJECT_BOUNDARY {
            let bucket = bucket_select(size);
            let bucket_min = bucket_to_size(bucket);
            let bucket_max = bucket_to_size(bucket + 1);
            assert!(bucket_min <= size);
            assert!(size < bucket_max);
            let lossage = (size - bucket_min) as f64 / bucket_max as f64;
            assert!(lossage * 4f64 <= 1f64);
        }
    }
}

pub const TINY_OBJECT_BOUNDARY: usize = 512;
const TINY_BUCKET_STEP: usize = 8;
pub const SMALL_OBJECT_BOUNDARY: usize = 8 * KB;
pub const LARGE_OBJECT_BOUNDARY: usize = 512 * KB;

const TINY_SMALL_BUCKETS: usize = TINY_OBJECT_BOUNDARY / TINY_BUCKET_STEP - 1;
pub const NONTINY_SMALL_BUCKETS: usize =
    semi_logarithmic_interval(TINY_OBJECT_BOUNDARY, SMALL_OBJECT_BOUNDARY);

pub const SMALL_BUCKETS: usize = TINY_SMALL_BUCKETS + NONTINY_SMALL_BUCKETS;
pub const LARGE_BUCKETS: usize =
    semi_logarithmic_interval(SMALL_OBJECT_BOUNDARY, LARGE_OBJECT_BOUNDARY);
pub const BUCKETS: usize = semi_logarithmic_interval(TINY_OBJECT_BOUNDARY, LARGE_OBJECT_BOUNDARY);
