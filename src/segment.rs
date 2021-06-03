use std::cell::UnsafeCell;
use std::pin::Pin;
use std::sync::Arc;
use std::{mem, ptr};

use parking_lot::Mutex;

use super::block::BlockHeader;
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
        } else if bucket < SMALL_BUCKETS + LARGE_BUCKETS {
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

static mut SEGMENT_REGISTRY: Option<Arc<Mutex<Vec<&'static SegmentHeader>>>> = None;

pub fn init_registry() { unsafe { SEGMENT_REGISTRY = Some(Arc::new(Mutex::new(Vec::new()))) }; }
pub fn registry() -> Arc<Mutex<Vec<&'static SegmentHeader>>> {
    unsafe { SEGMENT_REGISTRY.as_ref().unwrap_unchecked().clone() }
}

impl SegmentHeader {
    pub fn new(kind: SegmentType) -> Option<Vec<&'static BlockHeader>> {
        debug_assert!(match kind {
            SegmentType::Small | SegmentType::Large => true,
            _ => false,
        });
        let vm_region = VMRegion::new(4 * MB, 4 * MB).ok()?;
        unsafe {
            ptr::write(vm_region.base() as *mut SegmentHeader, SegmentHeader {
                block_shift: match kind {
                    SegmentType::Small => const { extrinsic_bsr(64 * KB) },
                    SegmentType::Large => const { extrinsic_bsr(4 * MB) },
                    SegmentType::Huge => unreachable!(),
                },
                kind,
                padding0_0: Default::default(),
                size: vm_region.size(),
                padding0: Default::default(),
            });
        }
        let header = unsafe { mem::transmute::<_, &'static mut SegmentHeader>(vm_region.base()) };

        let num_block_headers = header.num_blocks();
        let block_size = header.block_size();

        for i in 0..num_block_headers {
            let block_header_ptr = unsafe {
                mem::transmute::<_, *mut UnsafeCell<BlockHeader>>(header.block_header(i))
            };
            let block_body_offset = mem::size_of::<SegmentHeader>()
                + mem::size_of::<UnsafeCell<BlockHeader>>() * num_block_headers
                + i * block_size;
            let block_body_ptr = unsafe { vm_region.base().offset(block_body_offset as isize) };
            unsafe {
                ptr::write(
                    block_header_ptr,
                    UnsafeCell::new(BlockHeader::from_raw_parts(block_body_ptr)),
                );
            }
        }

        // update registry
        let registry = registry();
        registry.lock().push(header);

        Some({
            (0..num_block_headers)
                .map(|idx| unsafe { &*header.block_header(idx).get() })
                .collect::<Vec<_>>()
        })
    }

    pub fn block_shift(&self) -> usize { self.block_shift }
    pub fn block_size(&self) -> usize { 1 << self.block_shift }
    pub fn num_blocks(&self) -> usize { Self::num_blocks_for(self.kind) }
    pub const fn num_blocks_for(kind: SegmentType) -> usize {
        match kind {
            SegmentType::Small => 64,
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
    pub unsafe fn block_header(&'static self, index: usize) -> &'static UnsafeCell<BlockHeader> {
        self.as_segment().block_headers.get_unchecked(index)
    }
}
