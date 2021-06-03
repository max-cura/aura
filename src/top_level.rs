use std::default::Default;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::sync::Arc;

use parking_lot::Mutex;

use super::block::BlockHeader;
use super::bucket::*;
use super::segment::{SegmentHeader, SegmentType};

#[repr(C)]
pub struct TopLevel {
    small_empties: Mutex<Vec<&'static BlockHeader>>,
    small_buckets: [Mutex<Vec<&'static BlockHeader>>; SMALL_BUCKETS],
    large_empties: Mutex<Vec<&'static BlockHeader>>,
    large_buckets: [Mutex<Vec<&'static BlockHeader>>; LARGE_BUCKETS],
}

unsafe impl Sync for TopLevel {}

impl TopLevel {
    pub fn new() -> TopLevel {
        TopLevel {
            small_empties: Mutex::new(Vec::new()),
            small_buckets: <[Mutex<Vec<&'static BlockHeader>>; SMALL_BUCKETS] as Default>::default(
            ),
            large_empties: Mutex::new(Vec::new()),
            large_buckets: {
                let mut data: [MaybeUninit<Mutex<Vec<&'static BlockHeader>>>; LARGE_BUCKETS] =
                    unsafe { MaybeUninit::uninit().assume_init() };
                for elem in &mut data[..] {
                    unsafe { ptr::write(elem.as_mut_ptr(), Default::default()) };
                }
                unsafe { mem::transmute::<_, _>(data) }
            },
        }
    }

    pub fn indexed(&self, index: usize) -> &'_ Mutex<Vec<&'static BlockHeader>> {
        if index < SMALL_BUCKETS {
            unsafe { &self.small_buckets.get_unchecked(index) }
        } else if index < LARGE_BUCKETS {
            unsafe { &self.large_buckets.get_unchecked(index - SMALL_BUCKETS) }
        } else {
            panic!("bad (toplevel) bucket index: {}", index);
        }
    }
    pub fn which_empty(&self, index: usize) -> &'_ Mutex<Vec<&'static BlockHeader>> {
        if index < SMALL_BUCKETS {
            unsafe { &self.small_empties }
        } else if index < LARGE_BUCKETS {
            unsafe { &self.large_empties }
        } else {
            panic!("bad (toplevel) bucket index: {}", index);
        }
    }

    pub fn receive(&self, index: usize, header: &'static BlockHeader) {
        let mut guard = if header.allocated() == 0 {
            self.which_empty(index).lock()
        } else {
            self.indexed(index).lock()
        };
        guard.push(header);
    }

    pub fn request(&self, index: usize) -> Option<&'static BlockHeader> {
        let maybe_empties = unsafe { self.which_empty_unchecked(index).lock() };
        if !maybe_empties.is_empty() {
            let b = maybe_empties.pop();
            drop(maybe_empties);
            // unchecked because it doesn't need to be checked
            unsafe { b.as_ref().unwrap_unchecked() }.format(bucket_to_size(index));
            return b
        }
        let maybe_non_empties = unsafe { self.indexed_unchecked(index).lock() };
        if !maybe_non_empties.is_empty() {
            return maybe_non_empties.pop()
        } else {
            drop(maybe_non_empties);
        }

        let mut first = None;
        /* toplevel allocation */
        for block_header in SegmentHeader::new(SegmentType::from_bucket(index))?.into_iter() {
            match first {
                None => first = Some(block_header),
                _ => maybe_empties.push(block_header),
            };
        }
        unsafe { first.as_ref().unwrap_unchecked() }.format(bucket_to_size(index));
        first
    }
}

impl TopLevel {
    pub unsafe fn indexed_unchecked(&self, index: usize) -> &'_ Mutex<Vec<&'static BlockHeader>> {
        if index < SMALL_BUCKETS {
            &self.small_buckets.get_unchecked(index)
        } else {
            &self.large_buckets.get_unchecked(index - SMALL_BUCKETS)
        }
    }
    pub unsafe fn which_empty_unchecked(
        &self,
        index: usize,
    ) -> &'_ Mutex<Vec<&'static BlockHeader>> {
        if index < SMALL_BUCKETS {
            &self.small_empties
        } else {
            &self.large_empties
        }
    }

    pub unsafe fn try_get_block_header(&self, index: usize) -> Option<&'static BlockHeader> {
        let mut guard = self.indexed_unchecked(index).lock();
        guard.pop()
    }
}

static mut TOP_LEVEL: Option<Arc<TopLevel>> = None;

pub fn init_top_level() { unsafe { TOP_LEVEL = Some(Arc::new(TopLevel::new())) }; }
pub fn top_level() -> Arc<TopLevel> { unsafe { TOP_LEVEL.as_ref().unwrap_unchecked().clone() } }
