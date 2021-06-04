use std::default::Default;
use std::ptr;
use std::sync::atomic::*;

pub trait AnyFreeList {
    fn is_empty(&self) -> bool;
}

pub trait FreeListPush<T>: AnyFreeList {
    fn push(&mut self, ptr: *mut T);
    fn swap(&mut self, new_ptr: *mut T) -> *mut T;
}

pub trait FreeListPop<T>: AnyFreeList {
    fn pop(&mut self) -> *mut T;
}

#[repr(C)]
#[derive(Debug)]
pub struct AtomicPushFreeList<T>(AtomicPtr<T>);

impl<T> Default for AtomicPushFreeList<T> {
    fn default() -> AtomicPushFreeList<T> { AtomicPushFreeList::new() }
}

impl<T> AtomicPushFreeList<T> {
    pub fn new() -> AtomicPushFreeList<T> { AtomicPushFreeList(AtomicPtr::new(ptr::null_mut())) }
}

impl<T> AnyFreeList for AtomicPushFreeList<T> {
    fn is_empty(&self) -> bool { self.0.load(Ordering::SeqCst).is_null() }
}

impl<T: std::fmt::Debug> FreeListPush<T> for AtomicPushFreeList<T> {
    fn push(&mut self, ptr: *mut T) {
        let mut curr = self.0.load(Ordering::SeqCst);
        loop {
            unsafe { *{ ptr as *mut *mut T } = curr };
            #[cfg_attr(rustfmt, rustfmt_skip)]
            match self.0.compare_exchange_weak(curr, ptr, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => curr = actual,
            }
        }
        // println!("pushed {:#?} over {:#?}", ptr, unsafe { *(ptr as *mut *mut
        // T) });
    }

    fn swap(&mut self, new_ptr: *mut T) -> *mut T { self.0.swap(new_ptr, Ordering::SeqCst) }
}

#[repr(C)]
#[derive(Debug)]
pub struct BiFreeList<T>(*mut T);

impl<T> Default for BiFreeList<T> {
    fn default() -> BiFreeList<T> { BiFreeList(ptr::null_mut()) }
}

impl<T> BiFreeList<T> {
    pub fn new() -> BiFreeList<T> { BiFreeList(ptr::null_mut()) }
}

impl<T> AnyFreeList for BiFreeList<T> {
    fn is_empty(&self) -> bool { self.0.is_null() }
}

impl<T> FreeListPush<T> for BiFreeList<T> {
    fn push(&mut self, ptr: *mut T) {
        unsafe {
            *{ ptr as *mut *mut T } = self.0;
        }
        self.0 = ptr;
    }

    fn swap(&mut self, new_ptr: *mut T) -> *mut T {
        let r = self.0;
        self.0 = new_ptr;
        r
    }
}

impl<T> FreeListPop<T> for BiFreeList<T> {
    fn pop(&mut self) -> *mut T {
        let r = self.0;
        if !r.is_null() {
            self.0 = unsafe { *{ r as *mut *mut T } };
        }
        r
    }
}
