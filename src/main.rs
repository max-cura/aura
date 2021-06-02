#![allow(incomplete_features)]
#![allow(dead_code, unused_imports, unused_variables)]
#![feature(core_intrinsics)]
#![feature(nll)]
#![feature(cell_leak)]
#![feature(thread_id_value)]
#![feature(const_maybe_uninit_assume_init, inline_const, option_result_unwrap_unchecked)]

extern crate crossbeam_channel;
extern crate libc;
extern crate mach;
extern crate num_cpus;
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
mod top_level;
mod util;
pub mod vm;

// TODO uncomment
// thread_local! {
//s static LOCAL_HEAP: Cell<heap::Heap> = Cell::new(Heap::new());
// }

// /// VERY UNSAFE
// unsafe fn swap_heap(new_heap: heap::Heap) { LOCAL_HEAP.replace(new_heap); }

use std::cell::{RefCell, RefMut};
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::{mem, panic, process, thread};

use rand::prelude::*;

use self::block::BlockHeader;
use self::constants::KB;
use self::vm::{VMRegion, VirtualRegion};

enum EncounterCategorization {
    NotEncountered,
    Encountered,
    MultiplyEncountered,
}

fn main() {
    let vm_region = match VMRegion::new(64 * KB, 64 * KB) {
        Ok(r) => r,
        Err(e) => panic!("VMRegino allocation failed: {}", e),
    };
    let mut block_header = BlockHeader::from_raw_parts(vm_region.base());
    block_header.format(512);

    let iterations = 200000usize;
    let num_allocated = Arc::new(AtomicUsize::new(0));
    let num_freed = Arc::new(AtomicUsize::new(0));

    let num_threads = num_cpus::get();
    // let num_threads = 1usize;
    let mut handles = Vec::new();

    let mut block_header_cell = RefCell::new(block_header);

    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // invoke the default handler and exit the process
        orig_hook(panic_info);
        process::exit(1);
    }));

    for i in 0..num_threads {
        // we need multiple mutable references, and we need to put them in
        // different threads
        let borrow: RefMut<BlockHeader> = block_header_cell.borrow_mut();
        let block_header: &mut BlockHeader = unsafe {
            // transmute gives us 'static so we can send it into the thread
            // it *also* frees up the lifetime 'b in RefMut<'b, BlockHeader> so
            // the &'b block_header_cell that was created by borrow_mut descopes
            // so we can call undo_leak which needs &mut block_header_cell
            mem::transmute::<&mut BlockHeader, &'static mut BlockHeader>(RefMut::leak(borrow))
        };
        block_header_cell.undo_leak();

        let num_allocated = Arc::clone(&num_allocated);
        let num_freed = Arc::clone(&num_freed);
        handles.push(thread::spawn(move || {
            // debug: requires 4-core
            let color = match i {
                0 => "\x1b[31m",
                1 => "\x1b[32m",
                2 => "\x1b[33m",
                3 => "\x1b[34m",
                _ => unreachable!(),
            };
            let mut objects = Vec::<*mut u8>::new();
            for _ in 0..iterations {
                match thread_rng().gen::<u32>() % 2 {
                    0 => {
                        let obj = block_header.alloc();
                        if !obj.is_null() {
                            objects.push(obj);

                            num_allocated.fetch_add(1, Ordering::SeqCst);
                            io::stderr()
                                .lock()
                                .write_fmt(format_args!(
                                    "{}thread {} allocated {}\t({})\x1b[0m\n",
                                    color,
                                    thread::current().id().as_u64(),
                                    unsafe { obj.offset_from(block_header.base()) } / 512,
                                    objects
                                        .iter()
                                        .map(|n| format!(
                                            "{}",
                                            unsafe { n.offset_from(block_header.base()) } / 512
                                        ))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ))
                                .unwrap();
                        }
                    },
                    1 => {
                        if objects.len() > 0 {
                            let index = thread_rng().gen_range(0..objects.len());
                            let obj = objects.remove(index);

                            block_header.free(obj);
                            io::stderr()
                                .lock()
                                .write_fmt(format_args!(
                                    "{}thread {} freed {}\t({})\x1b[0m\n",
                                    color,
                                    thread::current().id().as_u64(),
                                    unsafe { obj.offset_from(block_header.base()) } / 512,
                                    objects
                                        .iter()
                                        .map(|n| format!(
                                            "{}",
                                            unsafe { n.offset_from(block_header.base()) } / 512
                                        ))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ))
                                .unwrap();

                            num_freed.fetch_add(1, Ordering::SeqCst);
                        }
                    },
                    _ => unreachable!(),
                }

                let dupes = objects.iter().all(|item| {
                    match objects.iter().fold(
                        EncounterCategorization::NotEncountered,
                        |accum, item2| {
                            if item == item2 {
                                match accum {
                                    EncounterCategorization::NotEncountered => {
                                        EncounterCategorization::Encountered
                                    },
                                    EncounterCategorization::Encountered => {
                                        EncounterCategorization::MultiplyEncountered
                                    },
                                    EncounterCategorization::MultiplyEncountered => {
                                        EncounterCategorization::MultiplyEncountered
                                    },
                                }
                            } else {
                                accum
                            }
                        },
                    ) {
                        EncounterCategorization::Encountered => true,
                        _ => false,
                    }
                });
                if !dupes && !objects.is_empty() {
                    panic!(
                        "thread {}: found duplicate objects: {}",
                        thread::current().id().as_u64(),
                        objects
                            .iter()
                            .map(|n| format!(
                                "{}",
                                unsafe { n.offset_from(block_header.base()) } / 512
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!(
        "allocated {}, freed {}",
        num_allocated.load(Ordering::Relaxed),
        num_freed.load(Ordering::Relaxed),
    );
}