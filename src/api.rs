use std::{mem, ptr};

use super::block::BlockHeader;
use super::segment::{self, SegmentHeader};
use super::{heap, top_level};
use crate::constants::MB;

pub fn aura_init() {
    // top_level::init_top_level();
    // segment::init_registry();
}
pub fn aura_alloc(size: usize) -> *mut u8 { heap::thread_heap().alloc(size) }
pub fn aura_free(object: *mut u8) { unsafe { find_block_for_object(object) }.free(object) }

unsafe fn find_block_for_object(object: *mut u8) -> &'static mut BlockHeader {
    let seg_header = mem::transmute::<_, &SegmentHeader>(object as usize & !(4 * MB - 1));
    let seg_offset = object as usize & (4 * MB - 1);
    let block_idx = (seg_offset / seg_header.block_size()) - 1;
    mem::transmute::<*mut BlockHeader, &'static mut BlockHeader>(
        seg_header.block_header(block_idx).get(),
    )
}
