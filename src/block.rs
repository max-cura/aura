use std::borrow::BorrowMut;
use std::cell::{RefCell, UnsafeCell};
use std::ops::Deref;
use std::sync::atomic::*;
use std::thread::{self, ThreadId};
use std::{mem, ptr};

use parking_lot::*;
use rand::distributions::{Distribution, Uniform};
use rand::prelude::*;
use rand::RngCore;
use rand_xoshiro::Xoshiro256StarStar;

use super::bucket::Bucket;
use super::free_list::{AnyFreeList, AtomicPushFreeList, BiFreeList, FreeListPop, FreeListPush};
use super::mesh::MeshMask;
use super::segment::SegmentHeader;
use super::top_level;
use crate::constants::{GB, KB, MB};

#[derive(Debug)]
pub struct AtomicTaggedPtr(AtomicUsize);

pub const PTR_TAG_MASK: usize = 0x7usize;

impl AtomicTaggedPtr {
    pub fn new<T>(p: *mut T) -> AtomicTaggedPtr { AtomicTaggedPtr(AtomicUsize::new(p as usize)) }

    pub fn set_ptr<T>(&mut self, p: *mut T) { self.0.store(p as usize, Ordering::SeqCst); }

    pub fn ptr<T>(&mut self) -> *mut T { (self.0.load(Ordering::SeqCst) & !PTR_TAG_MASK) as *mut T }

    pub fn tag(&self) -> u8 { (self.0.load(Ordering::SeqCst) & PTR_TAG_MASK) as u8 }

    pub fn set_tag(&mut self, new_tag: u8) {
        assert!(0 == (new_tag as usize & !PTR_TAG_MASK));
        let mut curr = self.0.load(Ordering::SeqCst);
        loop {
            let new_value = (curr & !PTR_TAG_MASK) | new_tag as usize;
            match self.0.compare_exchange_weak(curr, new_value, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => curr = actual,
            }
        }
    }
}

const BLOCK_FLAGS_NONE: u64 = 0u64;
const BLOCK_FLAGS_IS_ACTIVE: u64 = 1u64;

pub const BLOCK_FLAGS_MAYBE_FREE: u64 = 2u64;
const BLOCK_FLAGS_MAYBE_MESH: u64 = 4u64;

pub const BLOCK_FLAGS_FREE_LOCK: u64 = 8u64;

const MESH_TAG_NORMAL: u8 = 0;
const MESH_TAG_MESHING: u8 = 1;

#[repr(C)]
/// Invariant(flags'maybe-free => state in { currently active | empty })
/// Invariant(block in maybe-free-list => flags'maybe-free)
/// Invariant(block in maybe-mesh-list => flags'maybe-mesh)
/// Invariant(alloc_count == 0 => state in { empty }
///     |> alloc_count != 0 => state not in { empty })
/// Invariant(flags->not is_active => load alloc_count >= .alloc_count.)
pub struct BlockHeader {
    alloc_list: BiFreeList<u8>,
    free_list: BiFreeList<u8>,
    count: usize,
    object_size: usize,
    slow_interior: *mut u8,
    segment_idx: usize,
    pub next_in_bucket: *mut BlockHeader,
    padding0: [u64; 1],

    padding1: [u64; 3],
    pub_free_list: AtomicPushFreeList<u8>,
    bucket: *mut Bucket,
    tid: Option<ThreadId>,
    // Bucket::maybe_free_list
    free_mutex: RawMutex,
    padding1_0: [u8; 7],
    maybe_next_free: *mut BlockHeader,

    padding2: [u64; 3],
    pub flags: AtomicU64,
    alloc_count: AtomicUsize,
    // Invariant(state in { meshing } => mesh'tag'meshing
    //      |> mesh'tag'normal => state not in { meshing })
    mesh: AtomicTaggedPtr,
    mesh_mutex: RawMutex,
    padding2_0: [u8; 7],
    maybe_next_mesh: *mut BlockHeader,

    mesh_mask: MeshMask<64>,
}
impl std::fmt::Debug for BlockHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let selfp = self as *const BlockHeader;
        f.debug_struct("BlockHeader")
            .field("self", &selfp)
            .field("alloc_list", &self.alloc_list)
            .field("free_list", &self.free_list)
            .field("count", &self.count)
            .field("object_size", &self.object_size)
            .field("slow_interior", &self.slow_interior)
            .field("segment_idx", &self.segment_idx)
            .field("next_in_bucket", &self.next_in_bucket)
            .field("pub_free_list", &self.pub_free_list)
            .field(
                "bucket",
                if self.bucket.is_null() { &self.bucket } else { unsafe { &*self.bucket } },
            )
            .field("tid", &self.tid)
            .field("maybe_next_free", &self.maybe_next_free)
            .field("flags", &self.flags)
            .field("alloc_count", &self.alloc_count)
            .field("mesh", &self.mesh)
            .field("maybe_next_mesh", &self.maybe_next_mesh)
            // .field("mesh_mask", &self.mesh_mask)
            .finish()
    }
}

unsafe impl Send for BlockHeader {}
/// Required for certain global registries.
unsafe impl Sync for BlockHeader {}

impl BlockHeader {
    pub fn alloc(&mut self) -> *mut u8 {
        // Operation ordering:
        //  update alloc_count before allocating
        // This is to maintain: Invariant(alloc_list not null => alloc_count > 0)
        //
        // Meshing can take place whenever
        if self.alloc_list.is_empty() {
            // eprintln!("empty");
            if !self.free_list.is_empty() {
                self.alloc_list.swap(self.free_list.swap(ptr::null_mut()));
            } else if !self.pub_free_list.is_empty() {
                self.alloc_list.swap(self.pub_free_list.swap(ptr::null_mut()));
            }
            if self.alloc_list.is_empty() {
                return ptr::null_mut()
            }
        }
        let prev_cnt = self.alloc_count.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "{}T prev_cnt={}, now={} on {:#?}",
            thread::current().id().as_u64(),
            prev_cnt,
            self.alloc_count.load(Ordering::SeqCst),
            self as *const BlockHeader
        );
        // eprintln!("alloc on {:#?}", self as *const BlockHeader);
        // update mesh mask
        // eprintln!("getting addr...");
        let addr = self.alloc_list.pop();
        // eprintln!("addr = {:#?}", addr);
        let raw_offset = unsafe { addr.offset_from(self.slow_interior) };
        //eprintln!("raw_offset = {}", raw_offset);
        let offset = raw_offset as usize / self.object_size;
        self.mesh_mask.set(offset);
        // return allocated object
        addr
    }

    pub fn free(&mut self, obj: *mut u8) {
        debug_assert!(
            obj >= self.base()
                && obj
                    < unsafe {
                        self.base().offset((1usize << self.get_segment().block_shift()) as isize)
                        // self.base().offset(4 * KB as isize)
                    }
        );
        let is_pub = match self.tid {
            None => true,
            Some(block_tid) => block_tid.as_u64() != thread::current().id().as_u64(),
        };
        let prev_cnt2 = self.alloc_count.load(Ordering::SeqCst);
        let prev_cnt = self.alloc_count.fetch_sub(1, Ordering::SeqCst);
        eprintln!(
            "{}T prev_cnt={} (<- {}), now={} as {:#?}",
            thread::current().id().as_u64(),
            prev_cnt,
            prev_cnt2,
            self.alloc_count.load(Ordering::SeqCst),
            self as *const BlockHeader
        );

        if is_pub {
            // eprintln!("pub free");
            self.pub_free_list.push(obj);
        } else {
            // eprintln!("local free");
            self.free_list.push(obj);
        }
        if prev_cnt == 1 {
            // eprintln!(
            //     "{}T prev_cnt: {} ({:#?})",
            //     thread::current().id().as_u64(),
            //     prev_cnt,
            //     self as *const BlockHeader
            // );
            let mut flags_cache = self.flags.load(Ordering::SeqCst);
            if BLOCK_FLAGS_MAYBE_FREE != (flags_cache & BLOCK_FLAGS_MAYBE_FREE) {
                // if !(self.alloc_count.load(Ordering::SeqCst) == 0
                //     || BLOCK_FLAGS_IS_ACTIVE == flags_cache & BLOCK_FLAGS_IS_ACTIVE)
                // {
                //     eprintln!("{:#?}", self);
                //     panic!("assertion violation (that one ,_,)");
                // }
                let mut wrote = true;
                loop {
                    if BLOCK_FLAGS_MAYBE_FREE == (flags_cache & BLOCK_FLAGS_MAYBE_FREE) {
                        wrote = false;
                        break
                    }
                    while BLOCK_FLAGS_FREE_LOCK == flags_cache & BLOCK_FLAGS_FREE_LOCK {
                        flags_cache = self.flags.load(Ordering::SeqCst);
                        thread::yield_now();
                    }
                    match self.flags.compare_exchange_weak(
                        flags_cache,
                        flags_cache | BLOCK_FLAGS_MAYBE_FREE,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(actual) => flags_cache = actual,
                    }
                }
                if wrote {
                    if !self.bucket.is_null() {
                        unsafe { &mut *self.bucket }.maybe_free(self as *mut BlockHeader);
                    } else {
                        let top_level = top_level::get();
                        top_level.free(self);
                    }
                }
            }
        }
        // TODO: Meshing
    }

    pub fn allocated(&self) -> usize { self.alloc_count.load(Ordering::SeqCst) }
}

impl BlockHeader {
    pub fn prep_active(&mut self, bucket_ptr: *mut Bucket) {
        // need to update: tid, bucket (for now)
        self.tid = Some(thread::current().id());
        self.bucket = bucket_ptr;
        self.flags.fetch_or(BLOCK_FLAGS_IS_ACTIVE, Ordering::SeqCst);
    }

    pub fn prep_free(&mut self) {
        // NOT in freelist
        self.tid = None;
        self.bucket = ptr::null_mut();
        self.flags.fetch_and(!BLOCK_FLAGS_IS_ACTIVE, Ordering::SeqCst);
    }

    pub fn prep_inactive(&mut self) {
        // self.tid = None;
        // no need to set bucket
        self.flags.fetch_and(!BLOCK_FLAGS_IS_ACTIVE, Ordering::SeqCst);
    }
}

impl BlockHeader {
    pub fn _set_maybe_next_free(&mut self, new_ptr: *mut BlockHeader) {
        self.maybe_next_free = new_ptr;
    }
    pub fn _set_maybe_next_mesh(&mut self, new_ptr: *mut BlockHeader) {
        self.maybe_next_mesh = new_ptr;
    }

    pub fn _maybe_next_free(&self) -> *mut BlockHeader { self.maybe_next_free }
    pub fn _maybe_next_mesh(&self) -> *mut BlockHeader { self.maybe_next_mesh }
}

thread_local! (
    static THREAD_RNG: RefCell<Xoshiro256StarStar> = RefCell::new(Xoshiro256StarStar::from_seed({
        let mut data: <Xoshiro256StarStar as SeedableRng>::Seed = Default::default();
        thread_rng().fill_bytes(&mut data[0..]);
        data
    }))
);

impl BlockHeader {
    pub fn from_raw_parts(body: *mut u8, segment_idx: usize) -> BlockHeader {
        BlockHeader {
            alloc_list: BiFreeList::new(),
            free_list: BiFreeList::new(),
            count: 0,
            object_size: 0,
            slow_interior: body,
            segment_idx: segment_idx,
            next_in_bucket: ptr::null_mut(),
            padding0: Default::default(),
            padding1: Default::default(),
            pub_free_list: AtomicPushFreeList::new(),
            bucket: ptr::null_mut(),
            tid: None,
            free_mutex: <RawMutex as parking_lot::lock_api::RawMutex>::INIT,
            padding1_0: Default::default(),
            maybe_next_free: ptr::null_mut(),
            padding2: Default::default(),
            flags: AtomicU64::new(0),
            alloc_count: AtomicUsize::new(0),
            mesh: AtomicTaggedPtr::new::<u8>(ptr::null_mut()),
            mesh_mutex: <RawMutex as parking_lot::lock_api::RawMutex>::INIT,
            maybe_next_mesh: ptr::null_mut(),
            padding2_0: Default::default(),
            mesh_mask: MeshMask::new(),
        }
    }

    /// Format operation. Handles {alloc, free, pub_free}_list, count, and
    /// freelist setup.
    pub fn format(&mut self, osize: usize) -> *mut u8 {
        // THREAD_RNG.with(|rng| (*rng.borrow_mut()).next_u64());
        let block_size = 1usize << self.get_segment().block_shift();
        // eprintln!("Block size: {}", block_size);
        // let block_size = 4 * KB;
        self.count = block_size / osize;
        // eprintln!("block_size={}, osize={}, count={}", block_size, osize,
        // self.count);
        self.object_size = osize;

        let mut order: Vec<usize> = (0..self.count).collect();
        THREAD_RNG.with(|rng| order.shuffle(&mut *rng.borrow_mut()));
        // eprintln!(
        //     "Shuffled fmt vec: {}",
        //     order.iter().map(|n| format!("{}", n)).collect::<Vec<_>>().join(", ")
        // );
        let interior: *mut u8 = self.slow_interior;
        let mut curr: *mut *mut u8 =
            unsafe { interior.offset((order[0] * osize) as isize) } as *mut *mut u8;
        // let mut next: *mut *mut u8 = unsafe {
        // mem::MaybeUninit::uninit().assume_init() };
        let mut next: *mut *mut u8;

        self.alloc_list.swap(curr as *mut u8);
        self.free_list.swap(ptr::null_mut());
        self.pub_free_list.swap(ptr::null_mut());

        for i in 0..self.count - 1 {
            use std::io::Write;

            let tmp1 = unsafe { interior.offset((order[i + 1] * osize) as isize) };
            let tmp2 = tmp1 as *mut *mut u8;
            next = tmp2;
            // eprintln!(
            //     "#{}, curr=#{} ({:#?}), next=#{} ({:#?})... ",
            //     i,
            //     order[i],
            //     curr,
            //     order[i + 1],
            //     next
            // );
            unsafe { *curr = next as *mut u8 };
            curr = next;
        }
        unsafe { *curr = ptr::null_mut() };

        ptr::null_mut()
    }

    pub fn get_segment(&self) -> &SegmentHeader {
        let addr = unsafe { mem::transmute::<_, *mut u8>(self) as usize };
        unsafe { mem::transmute::<_, &SegmentHeader>((addr & !(4 * MB - 1)) as *mut u8) }
    }

    pub fn base(&self) -> *mut u8 { self.slow_interior }
}

impl BlockHeader {
    pub fn _count(&self) -> usize { self.count }
    pub fn _object_size(&self) -> usize { self.object_size }
    pub fn _segment_idx(&self) -> usize { self.segment_idx }
}

#[cfg(test)]
mod tests {
    use std::{mem, thread};

    use rand::prelude::*;

    use super::BlockHeader;
    use crate::constants::KB;
    use crate::segment::{SegmentHeader, SegmentType};
    use crate::vm::{VMRegion, VirtualRegion};
    use crate::{segment, top_level};

    enum EncounterCategorization {
        NotEncountered,
        Encountered,
        MultiplyEncountered,
    }

    //     #[test]
    //     fn stress_test_sequential() {
    //         let mut seg_blocks =
    // SegmentHeader::new(SegmentType::Small).unwrap();         let
    // block_header = unsafe {             mem::transmute::<*mut
    // BlockHeader, &mut BlockHeader>(
    // seg_blocks.pop().unwrap_unchecked().get(),             )
    //         };
    //         block_header.format(512);

    //         let mut objects = Vec::<*mut u8>::new();
    //         let iterations = 1_000_000usize;
    //         let mut num_allocated = 0usize;
    //         let mut num_freed = 0usize;
    //         let mut failed_allocations = 0usize;

    //         for _ in 0..iterations {
    //             match thread_rng().gen::<u32>() % 2 {
    //                 0 => {
    //                     let obj = block_header.alloc();
    //                     if !obj.is_null() {
    //                         objects.push(obj);

    //                         num_allocated += 1;
    //                     } else {
    //                         failed_allocations += 1;
    //                     }
    //                 },
    //                 1 => {
    //                     if objects.len() > 0 {
    //                         let index =
    // thread_rng().gen_range(0..objects.len());                         let
    // selection = objects.remove(index);

    //                         block_header.free(selection);

    //                         num_freed += 1;
    //                     }
    //                 },
    //                 _ => unreachable!(),
    //             }

    //             assert!(objects.iter().all(|item| {
    //                 match objects.iter().fold(
    //                     EncounterCategorization::NotEncountered,
    //                     |accum, item2| {
    //                         if item == item2 {
    //                             match accum {
    //                                 EncounterCategorization::NotEncountered
    // => {
    // EncounterCategorization::Encountered
    // },
    // EncounterCategorization::Encountered => {
    // EncounterCategorization::MultiplyEncountered
    // },
    // EncounterCategorization::MultiplyEncountered => {
    // EncounterCategorization::MultiplyEncountered
    // },                             }
    //                         } else {
    //                             accum
    //                         }
    //                     },
    //                 ) {
    //                     EncounterCategorization::Encountered => true,
    //                     _ => false,
    //                 }
    //             }));
    //         }

    //         assert!(num_allocated >= num_freed);
    //         assert!(num_allocated - num_freed <= block_header._count());
    //     }
}
