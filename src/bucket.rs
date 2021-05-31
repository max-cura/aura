use std::cell::UnsafeCell;
use std::default::Default;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::{intrinsics, mem, ptr};

use parking_lot::RawMutex;

use super::block::BlockHeader;
use super::free_list::{AtomicPushFreeList, FreeListPush};
use crate::constants::KB;

#[repr(C)]
pub struct Bucket<'a> {
    // size is implicit
    active: AtomicPtr<BlockHeader<'a>>,
    maybe_free_list: AtomicPtr<BlockHeader<'a>>,
    maybe_mesh_list: AtomicPtr<BlockHeader<'a>>,
    count: AtomicUsize,
}

// Primary path
impl<'a> Bucket<'a> {
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

    fn source_block(&mut self) -> *mut BlockHeader<'a> {}
}

// Infrastructure
impl<'a> Bucket<'a> {
    pub fn new() -> Bucket<'a> { Bucket::default() }

    /// Invariant(never bucket'maybe_free [@ block block])
    pub fn maybe_free(&mut self, block_header: *mut BlockHeader<'a>) {
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
impl<'a> Default for Bucket<'a> {
    fn default() -> Self {
        Bucket {
            active: AtomicPtr::new(ptr::null_mut()),
            maybe_free_list: AtomicPtr::new(ptr::null_mut()),
            maybe_mesh_list: AtomicPtr::new(ptr::null_mut()),
            count: AtomicUsize::new(0),
        }
    }
}

pub fn bucket_select_tiny(size: usize) -> usize {
    debug_assert!(size < TINY_OBJECT_BOUNDARY && size > 0);
    let size = if size >= 8 { size - 8 } else { size };
    size / TINY_BUCKET_STEP
}

pub fn bucket_select_small(size: usize) -> usize {
    debug_assert!(size >= TINY_OBJECT_BOUNDARY && size < SMALL_OBJECT_BOUNDARY);
    let ind = extrinsic_bsr(size) - GRAIN_LOG;
    let gran = size >> ind;
    let log_off = ind - extrinsic_bsr(TINY_OBJECT_BOUNDARY);
    GRAIN * log_off + gran
}

pub fn bucket_select_large(size: usize) -> usize {
    debug_assert!(size >= SMALL_OBJECT_BOUNDARY && size < LARGE_OBJECT_BOUNDARY);
    let ind = extrinsic_bsr(size) - GRAIN_LOG;
    let gran = size >> ind;
    let log_off = ind - extrinsic_bsr(SMALL_OBJECT_BOUNDARY);
    GRAIN * log_off + gran
}

const GRAIN: usize = 8;
const GRAIN_MASK: usize = 7;
const GRAIN_LOG: usize = extrinsic_bsr(GRAIN_MASK);

macro_rules! extrinsic_bsr_variant {
    ($func_name: ident, $typ: ty) => {
        const fn $func_name(x: $typ) -> usize {
            8usize * mem::size_of::<$typ>() - intrinsics::ctlz(x) as usize
        }
    };
}

extrinsic_bsr_variant!(extrinsic_bsr, usize);
extrinsic_bsr_variant!(extrinsic_bsr64, u64);
extrinsic_bsr_variant!(extrinsic_bsr32, u32);
extrinsic_bsr_variant!(extrinsic_bsr16, u16);
extrinsic_bsr_variant!(extrinsic_bsr8, u8);

const fn semi_logarithmic_interval(lower: usize, upper: usize) -> usize {
    let interval = extrinsic_bsr(upper / lower);
    GRAIN * (interval - GRAIN_LOG + 1)
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
