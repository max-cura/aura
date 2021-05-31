use std::intrinsics;
use std::iter::Iterator;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::sync::atomic::*;

#[repr(transparent)]
pub struct MeshMask<const N: usize>([AtomicU64; N]);

impl<const N: usize> MeshMask<N> {
    pub fn new() -> MeshMask<N> {
        MeshMask::<N>({
            let mut tmpdata: [MaybeUninit<AtomicU64>; N] =
                unsafe { MaybeUninit::uninit().assume_init() };
            for elem in &mut tmpdata {
                *elem = MaybeUninit::new(AtomicU64::new(0));
            }
            // normal transmute doesn't work here
            // tracking issue: https://github.com/rust-lang/rust/issues/61956
            unsafe { mem::transmute_copy::<_, [AtomicU64; N]>(&tmpdata) }
        })
    }

    pub fn set(&mut self, idx: usize) {
        self.0[idx / 64].fetch_or(1u64 << (idx % 64), Ordering::SeqCst);
    }
    pub fn reset(&mut self, idx: usize) {
        self.0[idx / 64].fetch_and(!(1u64 << (idx % 64)), Ordering::SeqCst);
    }
    pub fn test_reset(&mut self, idx: usize) -> bool {
        let mask = 1u64 << (idx % 64);
        0u64 != (self.0[idx / 64].fetch_and(!mask, Ordering::SeqCst) & mask)
    }

    pub fn clear(&mut self) {
        let mutator = unsafe { mem::transmute::<_, &mut [u64]>(&mut self.0[0..N]) };
        mutator.fill(0u64);
    }

    pub fn iter_mesh<'b>(&'b mut self) -> MeshIter<'b, N> { MeshIter::from_mesh(self) }
}

pub struct MeshIter<'a, const N: usize> {
    mesh: &'a mut MeshMask<N>,
    curr: usize,
    _marker: PhantomData<[char; N]>,
}

impl<'a, const N: usize> MeshIter<'a, N> {
    pub fn from_mesh<'b>(m: &'b mut MeshMask<N>) -> MeshIter<'b, N> {
        MeshIter { mesh: m, curr: 0, _marker: PhantomData }
    }
}

impl<'a, const N: usize> Iterator for MeshIter<'a, N> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let slice = unsafe { mem::transmute::<_, &[u64]>(&self.mesh.0[0..N]) };
        while self.curr < N {
            let seg = slice[self.curr];
            if 0u64 == seg {
                self.curr += 1;
            } else {
                let idx = 63usize - intrinsics::ctlz(seg) as usize;
                self.mesh.reset(idx);

                return Some(idx + 64 * self.curr)
            }
        }
        None
    }
}

pub fn meshes_with<const N: usize>(lhs: &MeshMask<N>, rhs: &MeshMask<N>) -> bool {
    let lhs = unsafe { mem::transmute::<_, &[u64]>(&lhs.0[0..N]) };
    let rhs = unsafe { mem::transmute::<_, &[u64]>(&rhs.0[0..N]) };
    let mut i = 0usize;
    let mut r = true;
    while i < N && r {
        r = if 0 != (lhs[i + 0] & rhs[i + 0]) { false } else { r };
        r = if 0 != (lhs[i + 1] & rhs[i + 1]) { false } else { r };
        r = if 0 != (lhs[i + 2] & rhs[i + 2]) { false } else { r };
        r = if 0 != (lhs[i + 3] & rhs[i + 3]) { false } else { r };
        r = if 0 != (lhs[i + 4] & rhs[i + 4]) { false } else { r };
        r = if 0 != (lhs[i + 5] & rhs[i + 5]) { false } else { r };
        r = if 0 != (lhs[i + 6] & rhs[i + 6]) { false } else { r };
        r = if 0 != (lhs[i + 7] & rhs[i + 7]) { false } else { r };
        i += 8;
    }
    r
}

pub fn should_mesh(count: usize, allocated: &[usize]) -> bool {
    unimplemented!();
}
