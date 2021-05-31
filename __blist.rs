#![allow(unused_imports)]

use std::{mem, ptr, thread, time};
use std::alloc::{self, Layout};
use std::sync::atomic::{Ordering, AtomicPtr};
use std::sync::Arc;
use std::cell::UnsafeCell;

#[repr(C)]
struct AtomicFreeList (AtomicPtr<u8>);

impl AtomicFreeList {
    pub fn new() -> AtomicFreeList {
        AtomicFreeList(AtomicPtr::new(ptr::null_mut()))
    }
    
    pub fn push(&mut self, ptr: *mut u8) {
        let mut curr = self.0.load(Ordering::Acquire);
        loop {
            unsafe { *(ptr as *mut *mut u8) = curr };
            match self.0.compare_exchange_weak(curr, ptr, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(actual) => { curr = actual; },
            }
        }
    }
    
    pub fn swap(&mut self, new_ptr: *mut u8) -> *mut u8 {
        self.0.swap(new_ptr, Ordering::AcqRel)
    }
}

#[repr(C)]
struct Block {
    alloc_list: *mut u8,
    free_list: AtomicFreeList,
    object_size: usize,
    block_size: usize,
    
    memory: *mut u8,
}

unsafe impl Sync for Block {}
unsafe impl Send for Block {}

impl Block {
    pub fn new(block_size: usize) -> Option<Block> {
        let m = unsafe { alloc::alloc(Layout::from_size_align(block_size,
                                                              block_size)
                                              .unwrap()) };
        if m.is_null() {
            None
        } else {
            let bl = Block {
                alloc_list: ptr::null_mut(),
                free_list: AtomicFreeList::new(),
                object_size: 0usize,
                block_size: block_size,
                memory: m
            };
            Some(bl)
        }
    }
    
    pub fn format(&mut self, osize: usize) {
        self.object_size = osize;
        let ocnt = self.block_size / osize;
        for i in 0usize..ocnt {
            let t = unsafe { self.memory.offset((i * osize) as isize) } as *mut *mut u8;
            let n = if i + 1 != ocnt {
                (unsafe { (t as *mut u8).offset(osize as isize) }) as *mut u8
            } else {
                ptr::null_mut()
            };
            unsafe { *t = n };
        }
        self.alloc_list = self.memory;
    }
    
    fn fast_alloc(&mut self) -> *mut u8 {
        let obj = self.alloc_list;
        let next = unsafe { *(obj as *mut *mut u8) };
        self.alloc_list = next;
        obj
    }
    
    pub fn alloc(&mut self) -> *mut u8 {
        if !self.alloc_list.is_null() {
            self.fast_alloc()
        } else {
            self.alloc_list = self.free_list.swap(ptr::null_mut());
            if !self.alloc_list.is_null() {
                self.fast_alloc()
            } else {
                ptr::null_mut()
            }
        }
    }
    
    pub fn free(&mut self, obj: *mut u8) {
        self.free_list.push(obj);
    }
}

fn main() {
    let mut block = Block::new(0x4000usize).unwrap();
    block.format(0x10usize);
    
    let c = Arc::new(block);
    
    let c1 = Arc::clone(&c);
    let t1 = thread::spawn(move || {
        for _ in 0..10 {
            println!("T1 allocated {:#?}",
            unsafe { &mut *(c1.as_ref() as *const Block as *mut Block) }.alloc());
            thread::sleep(time::Duration::from_millis(30));
        }
    });
    let c2 = c.clone();
    let t2 = thread::spawn(move || {
        for _ in 0..10 {
            println!("T2 allocated {:#?}",
            unsafe { &mut *(c2.as_ref() as *const Block as *mut Block) }.alloc());
            thread::sleep(time::Duration::from_millis(40));
        }
    });
    
    t1.join().unwrap();
    t2.join().unwrap();
}
