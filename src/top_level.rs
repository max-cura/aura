use std::cell::UnsafeCell;
use std::default::Default;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::block::{self, BlockHeader};
use super::bucket::*;
use super::segment::{SegmentHeader, SegmentType};

#[repr(C)]
pub struct TopLevel {
    empties: Mutex<Vec<&'static UnsafeCell<BlockHeader>>>,
    buckets: [Mutex<Vec<&'static UnsafeCell<BlockHeader>>>; BUCKETS],
    total_count: AtomicUsize,
}

/// For use with TopLevel::count
pub enum TopLevelBlockType {
    Empty,
    Total,
    Bucket(usize),
}

unsafe impl Sync for TopLevel {}
// for lazy_static!
unsafe impl Send for TopLevel {}

impl TopLevel {
    // New empty toplevel
    pub fn new() -> TopLevel {
        TopLevel {
            empties: Mutex::new(Vec::new()),
            buckets: {
                let mut data: [MaybeUninit<Mutex<Vec<&'static UnsafeCell<BlockHeader>>>>; BUCKETS] =
                    unsafe { MaybeUninit::uninit().assume_init() };
                for elem in &mut data[..] {
                    unsafe { ptr::write(elem.as_mut_ptr(), Default::default()) };
                }
                unsafe { mem::transmute::<_, _>(data) }
            },
            total_count: AtomicUsize::new(0),
        }
    }

    /// Number of block headers are there in a particular bucket.
    pub fn count(&self, block_type: TopLevelBlockType) -> usize {
        let which = match block_type {
            TopLevelBlockType::Empty => self.empties.lock(),
            TopLevelBlockType::Total => return self.total_count.load(Ordering::Relaxed),
            TopLevelBlockType::Bucket(bucket) => self.indexed(bucket).lock(),
        };
        which.len()
    }

    /// Get reference to mutex around bucket with index.
    pub fn indexed(&self, index: usize) -> &'_ Mutex<Vec<&'static UnsafeCell<BlockHeader>>> {
        if index < BUCKETS {
            unsafe { &self.buckets.get_unchecked(index) }
        } else {
            panic!("bad (toplevel) bucket index: {}", index);
        }
    }

    /// Free a block header that is already present in the top-level.
    pub fn free(&self, block_ref: &BlockHeader) {
        let index = bucket_select(block_ref._object_size());
        let mut bh_vec = unsafe { self.indexed_unchecked(index) }.lock();
        let vec_idx = bh_vec
            .iter()
            .position(|&item| {
                item.get() == unsafe { mem::transmute::<_, *mut BlockHeader>(block_ref) }
            })
            .unwrap();
        let header = bh_vec.remove(vec_idx);
        drop(bh_vec);

        self.receive(index, header);
    }

    /// Add a block header to the top-level.
    pub fn receive(&self, index: usize, header: &'static UnsafeCell<BlockHeader>) {
        let b_ref = unsafe { mem::transmute::<*mut BlockHeader, &mut BlockHeader>(header.get()) };
        let mut guard =
            if b_ref.allocated() == 0 { self.empties.lock() } else { self.indexed(index).lock() };
        guard.push(header);
        b_ref.flags.fetch_and(!block::BLOCK_FLAGS_FREE_LOCK, Ordering::SeqCst);
    }

    /// Request a block from bucket specified by index, otherwise a block sized
    /// appropriately to that bucket, if one can be got, otherwise (finally)
    /// None.
    pub fn request(&self, index: usize) -> Option<&'static UnsafeCell<BlockHeader>> {
        // Try to find a non-empty but correctly sized block
        let mut maybe_non_empties = unsafe { self.indexed_unchecked(index).lock() };
        if !maybe_non_empties.is_empty() {
            return maybe_non_empties.pop()
        } else {
            drop(maybe_non_empties);
        }

        // Try to find an empty block
        let mut maybe_empties = self.empties.lock();
        if !maybe_empties.is_empty() {
            let mut b = maybe_empties.pop();
            drop(maybe_empties);
            // format empty block
            let bh = unsafe {
                mem::transmute::<_, &'static mut BlockHeader>(
                    &mut *(*b.as_mut().unwrap_unchecked()).get(),
                )
            };
            bh.format(bucket_to_size(index + 1));
            return b
        }

        // couldn't find anything, so we allocate new blocks
        let mut first = None;
        // println!("bucket: {}", index);
        // println!(
        //     "TINY_BUCKETS={}, SMALL_BUCKETS={}, LARGE_BUCKETS={}, BUCKETS={}",
        //     TINY_SMALL_BUCKETS, SMALL_BUCKETS, LARGE_BUCKETS, BUCKETS
        // );
        for block_header in SegmentHeader::new(SegmentType::from_bucket(index))?.into_iter() {
            match first {
                None => first = Some(block_header),
                _ => maybe_empties.push(block_header),
            };
            self.total_count.fetch_add(1, Ordering::Relaxed);
        }
        drop(maybe_empties);

        // format empty block
        let bh = unsafe {
            mem::transmute::<_, &'static mut BlockHeader>(
                &mut *(*first.as_mut().unwrap_unchecked()).get(),
            )
        };
        bh.format(bucket_to_size(index + 1));
        first
    }
}

impl TopLevel {
    pub unsafe fn indexed_unchecked(
        &self,
        index: usize,
    ) -> &'_ Mutex<Vec<&'static UnsafeCell<BlockHeader>>> {
        &self.buckets.get_unchecked(index)
    }

    pub unsafe fn try_get_block_header(
        &self,
        index: usize,
    ) -> Option<&'static UnsafeCell<BlockHeader>> {
        let mut guard = self.indexed_unchecked(index).lock();
        guard.pop()
    }
}

lazy_static! {
    static ref TOP_LEVEL: Arc<TopLevel> = Arc::new(TopLevel::new());
}
// static mut TOP_LEVEL: Option<Arc<TopLevel>> = None;

// pub fn init_top_level() { unsafe { TOP_LEVEL =
// Some(Arc::new(TopLevel::new())) }; }
pub fn get() -> Arc<TopLevel> {
    // unsafe { TOP_LEVEL.as_ref().unwrap_unchecked().clone() }
    TOP_LEVEL.clone()
}
