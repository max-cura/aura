#[repr(C)]
pub struct SegmentHeader {
    // line 0
    block_shift: usize,
    kind: usize,
    padding0: [u64; 6],
}

impl SegmentHeader {
    pub fn block_shift(&self) -> usize { self.block_shift }
}
