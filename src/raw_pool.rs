// PROTOTYPE (CURRENTLY BROKEN) DO NOT USE.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, align_of, size_of};
use std::ops::{Deref, DerefMut, Drop};

use super::free_list::{AnyFreeList, BiList, FreeListPop, FreeListPush};
use super::vm::{self, VMRegion, VirtualRegion};

struct PoolSegment {
    inner: VMRegion,
    next: Option<Box<PoolSegment>>,
}

pub struct RawPool<T> {
    inner: Option<Box<PoolSegment>>,
    free_list: BiList<T>,
    bump: isize,
    segment_size: usize,
}

impl<T> RawPool<T> {
    const item_size: usize = size_of::<T>();
    const item_align: usize = vm::align_size(size_of::<T>(), align_of::<T>());

    pub fn new() -> RawPool<T> {
        RawPool {
            inner: None,
            free_list: BiList::new(),
            bump: -1,
            segment_size: vm::align_size(Self::item_size, 4 * vm::page_size()),
        }
    }

    pub fn allocate<'b>(&'b mut self, x: T) -> RawPoolGuard<'b, T> {
        if self.free_list.is_empty() && self.bump == -1 {
            let ps = Box::new(PoolSegment {
                inner: VMRegion::new(self.segment_size, vm::page_size()).unwrap(),
                next: mem::replace(&mut self.inner, None),
            });
            self.inner = Some(ps);
            self.bump = 0;
        }
        if self.free_list.is_empty() {
            let ps = self.inner.as_ref().unwrap();
            let tp = ps.inner.base() as *mut T;
            unsafe { *tp.offset(self.bump) = x };
            self.bump += 1;
            // safe to convert bump: bump is >= 0
            if self.bump as usize * Self::item_align > self.segment_size {
                self.bump = -1;
            }
            // tp
            // unsafe { mem::transmute::<_, &mut T>(tp) }
            RawPoolGuard::from_raw_parts(tp, self)
        } else {
            let tp = self.free_list.pop();
            unsafe { *tp = x };
            // tp
            // unsafe { mem::transmute::<_, &mut T>(tp) }
            RawPoolGuard::from_raw_parts(tp, self)
        }
    }

    pub fn free(&mut self, x: &mut T) {
        let x_ptr = x as *mut T;
        drop(x);
        self.free_list.push(x_ptr);
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use super::RawPool;

    #[test]
    fn test_raw_pool() {
        let mut rp: RawPool<usize> = RawPool::new();
        // let my_int: &mut usize = unsafe { mem::transmute::<_, &mut
        // usize>(rp.allocate(14)) };
        let mut my_int = rp.allocate(14);
        let my_int_ref = my_int.as_mut();
        assert_eq!(*my_int_ref, 14);
        *my_int_ref = 12;
        assert_eq!(*my_int_ref, 12);
        *my_int_ref = 12;
    }
}

pub struct RawPoolGuard<'pa, T> {
    inner: *mut T,
    owner: *mut RawPool<T>,
    _p: PhantomData<&'pa RawPool<T>>,
}

impl<'pa, T> RawPoolGuard<'pa, T> {
    pub fn from_raw_parts<'a>(ptr: *mut T, owner: &'a RawPool<T>) -> RawPoolGuard<'a, T> {
        RawPoolGuard {
            inner: ptr,
            owner: unsafe { mem::transmute::<_, *mut RawPool<T>>(owner) },
            _p: PhantomData,
        }
    }

    pub fn as_mut(&mut self) -> &'pa mut T { unsafe { mem::transmute::<_, &mut T>(self.inner) } }
    pub fn as_ref(&self) -> &'pa T { unsafe { mem::transmute::<_, &T>(self.inner) } }

    pub unsafe fn get(&mut self) -> *mut T { self.inner }
}

impl<'pa, T> Deref for RawPoolGuard<'pa, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target { self.as_ref() }
}

impl<'pa, T> DerefMut for RawPoolGuard<'pa, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { self.as_mut() }
}

impl<'pa, T> Drop for RawPoolGuard<'pa, T> {
    fn drop(&mut self) { unsafe { &mut *self.owner }.free(self.as_mut()) }
}
