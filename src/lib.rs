#![feature(core_intrinsics, nll)]
#![allow(dead_code, unused_imports, unused_variables)]

extern crate libc;
extern crate mach;
extern crate parking_lot;
extern crate rand;
extern crate rand_xoshiro;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    OutOfMemory,
    Generic(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::OutOfMemory => write!(f, "out of memory"),
            Error::Generic(s) => write!(f, "{}", s),
        }
    }
}

mod block;
// don't use:
//mod raw_pool;
pub mod constants {
    pub const KB: usize = 1usize << 10;
    pub const MB: usize = KB << 10;
    pub const GB: usize = MB << 10;
}
mod bucket;
mod free_list;
mod heap;
mod mesh;
mod segment;
mod shuffle;
pub mod vm;

// TODO uncomment
// thread_local! {
// static LOCAL_HEAP: Cell<heap::Heap> = Cell::new(Heap::new());
// }

// /// VERY UNSAFE
// unsafe fn swap_heap(new_heap: heap::Heap) { LOCAL_HEAP.replace(new_heap); }
