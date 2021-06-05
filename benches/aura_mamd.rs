#![feature(custom_test_frameworks)]
#![test_runner(criterion::runner)]
#![feature(thread_id_value)]

use std::{panic, process, thread};

use aura::api::{aura_alloc, aura_free};
use aura::constants::KB;
use criterion::Criterion;
use criterion_macro::criterion;
use rand::prelude::*;

fn criterion_bench_mamd_limit() -> Criterion { Criterion::default().sample_size(10) }

#[criterion(criterion_bench_mamd_limit())]
fn bench_aura_mamd(criterion: &mut Criterion) {
    criterion.bench_function("Aura MA/MD", |b| {
        b.iter(|| {
            // many allocator, many free site
            let iterations_per_thread = 100000000usize;

            let num_threads = num_cpus::get();
            let mut handles = Vec::new();

            let orig_hook = panic::take_hook();
            {
                panic::set_hook(Box::new(move |panic_info| {
                    // invoke the default handler and exit the process
                    orig_hook(panic_info);
                    process::exit(1);
                }));
            }

            let mut receivers = Vec::new();
            let mut senders = Vec::new();
            for _ in 0..num_threads {
                let (tx, rx) = crossbeam_channel::unbounded::<Box<u8>>();
                receivers.push(rx);
                senders.push(tx);
            }

            for _i in 0..num_threads {
                let thread_rx = receivers.pop().unwrap();
                let thread_tx_bank = senders.iter().map(|tx| tx.clone()).collect::<Vec<_>>();
                // eprintln!("Starting thread {}", i);
                handles.push(thread::spawn(move || {
                    // debug: requires 4-core
                    // let color = match _i {
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

                                    // eprintln!(
                                    //     "{}t{}a{:#?}\t({})\x1b[0m",
                                    //     color,
                                    //     thread::current().id().as_u64(),
                                    //     obj,
                                    //     objects.len()
                                    // );
                                } else {
                                }
                            },
                            1 => {
                                if objects.len() > 0 {
                                    let index = thread_rng().gen_range(0..objects.len());
                                    let obj = objects.remove(index);

                                    // eprintln!(
                                    //     "{}t{}f{:#?}\t({})\x1b[0m",
                                    //     color,
                                    //     thread::current().id().as_u64(),
                                    //     obj,
                                    //     objects.len()
                                    // );
                                    aura_free(obj);
                                }
                            },
                            2 => {
                                if objects.len() > 0 {
                                    let index = thread_rng().gen_range(0..objects.len());
                                    let obj = objects.remove(index);
                                    loop {
                                        let recv_idx = thread_rng().gen_range(0..num_threads);
                                        // eprintln!(
                                        //     "{}t{}s{:#?} -> {}\x1b[0m",
                                        //     color,
                                        //     thread::current().id().as_u64(),
                                        //     obj,
                                        //     recv_idx
                                        // );
                                        match thread_tx_bank[recv_idx]
                                            .send(unsafe { Box::from_raw(obj) })
                                        {
                                            Ok(_) => break,
                                            Err(_) => continue,
                                        }
                                    }
                                }
                            },
                            3 => {
                                thread_rx.try_iter().for_each(|obj| {
                                    let obj = Box::into_raw(obj);
                                    // eprintln!(
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
                    }
                    for tx in thread_tx_bank.into_iter() {
                        drop(tx);
                    }
                    thread_rx
                        .iter()
                        .chain(objects.into_iter().map(|p| unsafe { Box::from_raw(p) }))
                        .for_each(|obj| {
                            let obj = Box::into_raw(obj);
                            // eprintln!(
                            //     "{}t{}f end {:#?}\x1b[0m",
                            //     color,
                            //     thread::current().id().as_u64(),
                            //     obj
                            // );
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
        })
    });
}
