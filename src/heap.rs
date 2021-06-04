use std::cell::UnsafeCell;
use std::mem::{self, MaybeUninit};
use std::ptr;

use super::bucket::{bucket_select, Bucket, BUCKETS};

#[repr(C)]
pub struct Heap {
    buckets: [UnsafeCell<Bucket>; BUCKETS],
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            buckets: {
                let mut data: [MaybeUninit<UnsafeCell<Bucket>>; BUCKETS] =
                    unsafe { MaybeUninit::uninit().assume_init() };
                for elem in &mut data[..] {
                    unsafe { ptr::write(elem.as_mut_ptr(), Default::default()) };
                }
                unsafe { mem::transmute::<_, _>(data) }
            },
        }
    }

    pub fn alloc(&self, size: usize) -> *mut u8 {
        let bucket_idx = bucket_select(size);
        // println!(
        //     "size={}, bucket={} [{}, {})",
        //     size,
        //     bucket_idx,
        //     super::bucket::bucket_to_size(bucket_idx),
        //     super::bucket::bucket_to_size(bucket_idx + 1)
        // );
        unsafe { &mut *self.buckets.get_unchecked(bucket_idx).get() }.alloc(bucket_idx)
    }
}

thread_local! {
    pub static THREAD_HEAP: Heap = Heap::new();
}

pub fn thread_heap() -> &'static Heap {
    THREAD_HEAP.with(|heap| unsafe { mem::transmute::<&'_ Heap, &'static Heap>(heap) })
}
