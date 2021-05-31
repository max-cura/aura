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
use crate::constants::{GB, KB, MB};

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

const BLOCK_FLAGS_MAYBE_FREE: u64 = 2u64;
const BLOCK_FLAGS_MAYBE_MESH: u64 = 4u64;

const MESH_TAG_NORMAL: u8 = 0;
const MESH_TAG_MESHING: u8 = 1;

#[repr(C)]
// 'a is the lifetime of the bucket
/// Invariant(flags'maybe-free => state in { currently active | empty })
/// Invariant(block in maybe-free-list => flags'maybe-free)
/// Invariant(block in maybe-mesh-list => flags'maybe-mesh)
/// Invariant(alloc_count == 0 => state in { empty }
///     |> alloc_count != 0 => state not in { empty })
pub struct BlockHeader<'a> {
    alloc_list: BiFreeList<u8>,
    free_list: BiFreeList<u8>,
    count: usize,
    object_size: usize,
    slow_interior: *mut u8,
    padding0: [u64; 3],

    padding1: [u64; 4],
    pub_free_list: AtomicPushFreeList<u8>,
    bucket: Option<&'a UnsafeCell<Bucket<'a>>>,
    tid: Option<ThreadId>,
    // Bucket::maybe_free_list
    //free_mutex: RawMutex,
    maybe_next_free: *mut BlockHeader<'a>,

    padding2: [u64; 3],
    flags: AtomicU64,
    alloc_count: AtomicUsize,
    // Invariant(state in { meshing } => mesh'tag'meshing
    //      |> mesh'tag'normal => state not in { meshing })
    mesh: AtomicTaggedPtr,
    mesh_mutex: RawMutex,
    padding2_0: [u8; 7],
    maybe_next_mesh: *mut BlockHeader<'a>,

    mesh_mask: MeshMask<128>,
    // at: 12 lines total
}

impl<'a> BlockHeader<'a> {
    pub fn alloc(&mut self) -> *mut u8 {
        // Operation ordering:
        //  update alloc_count before allocating
        // This is to maintain: Invariant(alloc_list not null => alloc_count > 0)
        //
        // Meshing can take place whenever
        if self.alloc_list.is_empty() {
            if !self.free_list.is_empty() {
                self.alloc_list.swap(self.free_list.swap(ptr::null_mut()));
            } else if !self.pub_free_list.is_empty() {
                self.alloc_list.swap(self.pub_free_list.swap(ptr::null_mut()));
            }
            if self.alloc_list.is_empty() {
                return ptr::null_mut()
            }
        }
        self.alloc_count.fetch_add(1, Ordering::SeqCst);
        // update mesh mask
        let addr = self.alloc_list.pop();
        let offset = unsafe { addr.offset_from(self.slow_interior) as usize } / self.object_size;
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
                    }
        );
        let is_pub = match self.tid {
            None => false,
            Some(block_tid) => block_tid == thread::current().id(),
        };
        let prev_cnt = self.alloc_count.fetch_sub(1, Ordering::SeqCst);
        if is_pub {
            self.pub_free_list.push(obj);
        } else {
            self.free_list.push(obj);
        }
        if prev_cnt == 1 {
            let mut flags_cache = self.flags.load(Ordering::SeqCst);
            if BLOCK_FLAGS_MAYBE_FREE != (flags_cache & BLOCK_FLAGS_MAYBE_FREE) {
                loop {
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
                self.bucket.unwrap();
            }
        }
        // handle meshing...
        else {
        }
    }
}

impl<'a> BlockHeader<'a> {
    pub fn _set_maybe_next_free(&mut self, new_ptr: *mut BlockHeader<'a>) {
        self.maybe_next_free = new_ptr;
    }
    pub fn _set_maybe_next_mesh(&mut self, new_ptr: *mut BlockHeader<'a>) {
        self.maybe_next_mesh = new_ptr;
    }

    pub fn _maybe_next_free(&self) -> *mut BlockHeader<'a> { self.maybe_next_free }
    pub fn _maybe_next_mesh(&self) -> *mut BlockHeader<'a> { self.maybe_next_mesh }
}

thread_local! (
    static THREAD_RNG: RefCell<Xoshiro256StarStar> = RefCell::new(Xoshiro256StarStar::from_seed({
        let mut data: <Xoshiro256StarStar as SeedableRng>::Seed = Default::default();
        thread_rng().fill_bytes(&mut data[0..]);
        data
    }))
);

impl<'a> BlockHeader<'a> {
    pub fn transfer<'b>(self, new_bucket: &'b UnsafeCell<Bucket<'b>>) -> BlockHeader<'b> {
        unsafe { mem::transmute::<BlockHeader<'a>, BlockHeader<'b>>(self) }
    }

    pub fn from_raw_parts<'b>(body: *mut u8) -> BlockHeader<'a> {
        BlockHeader {
            alloc_list: BiFreeList::new(),
            free_list: BiFreeList::new(),
            count: 0,
            object_size: 0,
            slow_interior: body,
            padding0: Default::default(),
            padding1: Default::default(),
            pub_free_list: AtomicPushFreeList::new(),
            bucket: None,
            tid: None,
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
        self.count = block_size / osize;
        self.object_size = osize;

        let mut order: Vec<usize> = (0..self.count).collect();
        THREAD_RNG.with(|rng| order.shuffle(&mut *rng.borrow_mut()));
        let interior = self.slow_interior;
        let mut curr: *mut *mut u8 =
            unsafe { interior.offset((order[0] * osize) as isize) } as *mut *mut u8;
        let mut next: *mut *mut u8;

        self.alloc_list.swap(curr as *mut u8);
        self.free_list.swap(ptr::null_mut());
        self.pub_free_list.swap(ptr::null_mut());

        for i in 0..self.count - 1 {
            next = unsafe { interior.offset((order[i + 1] * osize) as isize) } as *mut *mut u8;
            unsafe { *curr = next as *mut u8 };
            curr = next;
        }

        ptr::null_mut()
    }

    pub fn get_segment(&self) -> &SegmentHeader {
        let addr = unsafe { mem::transmute::<_, *mut u8>(self) as usize };
        unsafe { mem::transmute::<_, &SegmentHeader>((addr & !(4 * MB - 1)) as *mut u8) }
    }

    pub fn base(&self) -> *mut u8 { self.slow_interior }
}
