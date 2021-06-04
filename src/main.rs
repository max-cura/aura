#![allow(incomplete_features)]
#![allow(dead_code, unused_imports, unused_variables)]
#![feature(core_intrinsics)]
#![feature(nll)]
#![feature(cell_leak)]
#![feature(thread_id_value)]
#![feature(const_maybe_uninit_assume_init, inline_const, const_generics, const_evaluatable_checked)]
#![feature(option_result_unwrap_unchecked)]
#![feature(format_args_nl)]

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
    pub const KB: usize = 1024;
    pub const MB: usize = KB * 1024;
    pub const GB: usize = MB * 1024;
}
mod api;
mod bucket;
mod free_list;
mod heap;
mod mesh;
mod segment;
mod shuffle;
mod top_level;
mod util;
mod vm;

use std::cell::{RefCell, RefMut};
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::{mem, panic, process, thread};

pub use api::{aura_alloc, aura_free, aura_init};
use rand::prelude::*;

use self::block::BlockHeader;
use self::constants::KB;
use self::segment::{SegmentHeader, SegmentType};
use self::vm::{VMRegion, VirtualRegion};

enum EncounterCategorization {
    NotEncountered,
    Encountered,
    MultiplyEncountered,
}

fn main() {
    print!("Performing initialization...");
    aura_init();
    println!("done");

    print!("Testing many allocator threads, one deallocator thread...");

    // many allocators, one free site
    let (tx, rx) = crossbeam_channel::unbounded::<Box<u8>>();
    let mut handles = Vec::new();
    for i in 0..4 {
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..9 {
                let obj = aura_alloc(16);
                tx.send(unsafe { Box::from_raw(obj) }).unwrap();
            }
        }));
    }
    drop(tx);
    for handle in handles.into_iter() {
        handle.join().unwrap();
    }
    for alloc in rx.iter() {
        aura_free(Box::into_raw(alloc));
    }
    println!("ok");

    print!("Testing one allocator thread, many deallocator threads...");

    // one allocator, many free sites
    let mut allocs = Vec::new();
    for _ in 0..36 {
        let obj = aura_alloc(8 * KB - 1);
        unsafe { *obj = 3 };
        allocs.push(obj);
    }
    let mut handles = Vec::new();
    for _ in 0..36 {
        // need something send
        let object = unsafe { Box::from_raw(allocs.pop().unwrap()) };
        handles.push(thread::spawn(move || {
            aura_free(Box::into_raw(object));
        }));
    }
    for handle in handles.into_iter() {
        handle.join().unwrap();
    }
    for _ in 0..36 {
        let obj = aura_alloc(8 * KB - 1);
        unsafe { *obj = 4 };
        allocs.push(obj);
    }
    for _ in 0..36 {
        aura_free(allocs.pop().unwrap());
    }
    println!("ok");

    println!("Testing many allocator threads, many deallocator threads...");
    // many allocator, many free site
    let iterations_per_thread = 100000000usize;
    let num_allocated = Arc::new(AtomicUsize::new(0));
    let num_failed = Arc::new(AtomicUsize::new(0));
    let num_freed = Arc::new(AtomicUsize::new(0));

    let num_threads = num_cpus::get();
    let mut handles = Vec::new();

    let orig_hook = panic::take_hook();
    {
        let num_allocated = Arc::clone(&num_allocated);
        let num_freed = Arc::clone(&num_freed);
        let num_failed = Arc::clone(&num_failed);
        panic::set_hook(Box::new(move |panic_info| {
            // invoke the default handler and exit the process
            orig_hook(panic_info);
            println!(
                "failed: panicked (allocated {}, failed {} allocations,  freed {})",
                num_allocated.load(Ordering::Relaxed),
                num_failed.load(Ordering::Relaxed),
                num_freed.load(Ordering::Relaxed),
            );
            process::exit(1);
        }));
    }

    let mut receivers = Vec::new();
    let mut senders = Vec::new();
    for i in 0..num_threads {
        let (tx, rx) = crossbeam_channel::unbounded::<Box<u8>>();
        receivers.push(rx);
        senders.push(tx);
    }

    for i in 0..num_threads {
        let num_allocated = Arc::clone(&num_allocated);
        let num_freed = Arc::clone(&num_freed);
        let num_failed = Arc::clone(&num_failed);
        let thread_rx = receivers.pop().unwrap();
        let thread_tx_bank = senders.iter().map(|tx| tx.clone()).collect::<Vec<_>>();
        // println!("Starting thread {}", i);
        handles.push(thread::spawn(move || {
            // debug: requires 4-core
            // let color = match i {
            //     0 => "\x1b[31m",
            //     1 => "\x1b[32m",
            //     2 => "\x1b[33m",
            //     3 => "\x1b[34m",
            //     _ => unreachable!(),
            // };
            let mut objects = Vec::<*mut u8>::new();
            for _ in 0..iterations_per_thread {
                match thread_rng().gen_range(0..4) {
                    0 => {
                        let obj = aura_alloc(thread_rng().gen_range(1..8 * KB));
                        if !obj.is_null() {
                            objects.push(obj);

                            num_allocated.fetch_add(1, Ordering::SeqCst);
                            // println!(
                            //     "{}t{}a{:#?}\t({})\x1b[0m",
                            //     color,
                            //     thread::current().id().as_u64(),
                            //     obj,
                            //     objects.len()
                            // );
                        } else {
                            num_failed.fetch_add(1, Ordering::SeqCst);
                        }
                    },
                    1 => {
                        if objects.len() > 0 {
                            let index = thread_rng().gen_range(0..objects.len());
                            let obj = objects.remove(index);

                            // println!(
                            //     "{}t{}f{:#?}\t({})\x1b[0m",
                            //     color,
                            //     thread::current().id().as_u64(),
                            //     obj,
                            //     objects.len()
                            // );
                            aura_free(obj);

                            num_freed.fetch_add(1, Ordering::SeqCst);
                        }
                    },
                    2 => {
                        if objects.len() > 0 {
                            let index = thread_rng().gen_range(0..objects.len());
                            let obj = objects.remove(index);
                            loop {
                                let recv_idx = thread_rng().gen_range(0..num_threads);
                                // println!(
                                //     "{}t{}s{:#?} -> {}\x1b[0m",
                                //     color,
                                //     thread::current().id().as_u64(),
                                //     obj,
                                //     recv_idx
                                // );
                                match thread_tx_bank[recv_idx].send(unsafe { Box::from_raw(obj) }) {
                                    Ok(_) => break,
                                    Err(_) => continue,
                                }
                            }
                        }
                    },
                    3 => {
                        thread_rx.try_iter().for_each(|obj| {
                            num_freed.fetch_add(1, Ordering::SeqCst);
                            let obj = Box::into_raw(obj);
                            // println!(
                            //     "{}t{}f pub {:#?}\x1b[0m",
                            //     color,
                            //     thread::current().id().as_u64(),
                            //     obj
                            // );
                            aura_free(obj);
                        });
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
                        objects.iter().map(|n| format!("{:#?}", n)).collect::<Vec<_>>().join(", ")
                    );
                }
            }
            for tx in thread_tx_bank.into_iter() {
                drop(tx);
            }
            thread_rx
                .iter()
                .chain(objects.into_iter().map(|p| unsafe { Box::from_raw(p) }))
                .for_each(|obj| {
                    let obj = Box::into_raw(obj);
                    // println!(
                    //     "{}t{}f end {:#?}\x1b[0m",
                    //     color,
                    //     thread::current().id().as_u64(),
                    //     obj
                    // );
                    num_freed.fetch_add(1, Ordering::SeqCst);
                    aura_free(obj);
                });
        }));
    }
    for i in senders.into_iter() {
        drop(i);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!(
        "ok (allocated {}, failed {} allocations,  freed {})",
        num_allocated.load(Ordering::Relaxed),
        num_failed.load(Ordering::Relaxed),
        num_freed.load(Ordering::Relaxed),
    );

    println!(
        "toplevel count (empty): {}/{}",
        top_level::get().count(top_level::TopLevelBlockType::Empty),
        top_level::get().count(top_level::TopLevelBlockType::Total)
    );
}
