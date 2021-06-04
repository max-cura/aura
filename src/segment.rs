use std::cell::UnsafeCell;
use std::pin::Pin;
use std::sync::Arc;
use std::{mem, ptr};

use parking_lot::Mutex;

use super::block::BlockHeader;
use super::bucket::*;
use super::constants::{KB, MB};
use super::util::extrinsic_bsr;
use super::vm::{VMRegion, VirtualRegion};

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum SegmentType {
    Small,
    Large,
    Huge,
}

impl SegmentType {
    pub fn from_bucket(bucket: usize) -> SegmentType {
        if bucket < SMALL_BUCKETS {
            SegmentType::Small
        } else if bucket < BUCKETS {
            SegmentType::Large
        } else {
            SegmentType::Huge
        }
    }
}

#[repr(C)]
pub struct SegmentHeader {
    // line 0
    block_shift: usize,
    kind: SegmentType,
    padding0_0: [u8; 7],
    size: usize,
    padding0: [u64; 5],
}

#[repr(C)]
struct OpaqueExtendedSegmentHeader {
    header: SegmentHeader,
    block_headers: [UnsafeCell<BlockHeader>],
}

lazy_static! {
    static ref SEGMENT_REGISTRY: Arc<Mutex<Vec<&'static SegmentHeader>>> =
        Arc::new(Mutex::new(Vec::new()));
}

// static mut SEGMENT_REGISTRY: Option<Arc<Mutex<Vec<&'static SegmentHeader>>>>
// = None;

// pub fn init_registry() { unsafe { SEGMENT_REGISTRY =
// Some(Arc::new(Mutex::new(Vec::new()))) }; }
pub fn registry() -> Arc<Mutex<Vec<&'static SegmentHeader>>> {
    // unsafe { SEGMENT_REGISTRY.as_ref().unwrap_unchecked().clone() }
    SEGMENT_REGISTRY.clone()
}

impl SegmentHeader {
    pub fn new(kind: SegmentType) -> Option<Vec<&'static UnsafeCell<BlockHeader>>> {
        debug_assert!(match kind {
            SegmentType::Small | SegmentType::Large => true,
            _ => false,
        });
        // match kind {
        // SegmentType::Small => println!("Creating SMALL segment"),
        // SegmentType::Large => println!("Creating LARGE segment"),
        // _ => (),
        // };
        let vm_region = VMRegion::new(4 * MB, 4 * MB).ok()?;
        unsafe {
            ptr::write(vm_region.base() as *mut SegmentHeader, SegmentHeader {
                block_shift: match kind {
                    SegmentType::Small => const { extrinsic_bsr(64 * KB - 1) },
                    SegmentType::Large => const { extrinsic_bsr(4 * MB - 1) },
                    SegmentType::Huge => unreachable!(),
                },
                kind,
                padding0_0: Default::default(),
                size: vm_region.size(),
                padding0: Default::default(),
            });
        }
        let header: &'static mut SegmentHeader =
            unsafe { mem::transmute::<_, _>(vm_region.base()) };

        let num_block_headers = header.num_blocks();
        let block_size = header.block_size();
        // let begin = mem::size_of::<SegmentHeader>()
        //     + mem::size_of::<UnsafeCell<BlockHeader>>() * num_block_headers;
        // println!("segment begin offset: {}", begin);

        for i in 0..num_block_headers {
            let block_header_ptr = unsafe {
                mem::transmute::<_, *mut UnsafeCell<BlockHeader>>(header.block_header(i))
            };
            // println!("Creating block #{} in segment", i);
            // let block_body_offset = mem::size_of::<SegmentHeader>()
            //     + mem::size_of::<UnsafeCell<BlockHeader>>() * num_block_headers
            //     + i * block_size;
            let block_body_offset = match kind {
                SegmentType::Small => (i + 1) * block_size,
                SegmentType::Large => unimplemented!(),
                SegmentType::Huge => unimplemented!(),
            };
            let block_body_ptr = unsafe { vm_region.base().offset(block_body_offset as isize) };
            unsafe {
                ptr::write(
                    block_header_ptr,
                    UnsafeCell::new(BlockHeader::from_raw_parts(block_body_ptr, i)),
                );
            }
        }

        // update registry
        let registry = registry();
        registry.lock().push(header);

        Some({
            (0..num_block_headers)
                .map(|idx| unsafe { header.block_header(idx) })
                .collect::<Vec<_>>()
        })
    }

    pub fn block_shift(&self) -> usize { self.block_shift }
    pub fn block_size(&self) -> usize { 1 << self.block_shift }
    pub fn num_blocks(&self) -> usize { Self::num_blocks_for(self.kind) }
    pub const fn num_blocks_for(kind: SegmentType) -> usize {
        match kind {
            SegmentType::Small => 63,
            SegmentType::Large => 1,
            SegmentType::Huge => 1,
        }
    }
    unsafe fn as_segment(&self) -> &'_ OpaqueExtendedSegmentHeader {
        // Segment.header is at offset 0 (guaranteed by repr(C)) in Segment so
        // we can do this:
        let this = self as *const SegmentHeader;
        let slice = std::slice::from_raw_parts(this as *const (), self.size);
        mem::transmute::<_, &'_ OpaqueExtendedSegmentHeader>(
            slice as *const [()] as *const OpaqueExtendedSegmentHeader,
        )
    }
    pub unsafe fn block_header(&self, index: usize) -> &'static UnsafeCell<BlockHeader> {
        // need to go from '1 to 'static
        // 'static to 'static WILL NOT WORK: the lifetime will not be perceived
        // as having ended after borrow-drop
        mem::transmute::<&SegmentHeader, &'static SegmentHeader>(self)
            .as_segment()
            .block_headers
            .get_unchecked(index)
    }
}
