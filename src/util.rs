use std::{intrinsics, mem};

macro_rules! extrinsic_bsr_variant {
    ($func_name: ident, $typ: ty) => {
        pub const fn $func_name(x: $typ) -> usize {
            8usize * mem::size_of::<$typ>() - intrinsics::ctlz(x) as usize
        }
    };
}

extrinsic_bsr_variant!(extrinsic_bsr, usize);
extrinsic_bsr_variant!(extrinsic_bsr64, u64);
extrinsic_bsr_variant!(extrinsic_bsr32, u32);
extrinsic_bsr_variant!(extrinsic_bsr16, u16);
extrinsic_bsr_variant!(extrinsic_bsr8, u8);
