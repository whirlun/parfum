use parfum::thread;
use std::time::Duration;

fn main() {
    parfum::thread::setup_preemption();
    // Spawn a closure using the new spawn_fn API!
    // parfum::thread::spawn_fn(|| {
    //     for i in 1..=100 {
    //         println!("[closure green thread] i = {}", i);
    //         std::thread::sleep(Duration::from_millis(200));
    //     }
    //     println!("[closure green thread] done!");
    // });

    // Test explicit yield and preemption
    for tid in 0..3 {
        parfum::thread::spawn_fn(move || {
            let mut i = 0;
            loop {
                // Preemption check at the top
                parfum::thread::check_preemption();
                println!("[green thread {}] iteration {}", tid, i);
                i += 1;
                if i % 3 == 0 {
                    println!("[green thread {}] yielding", tid);
                    parfum::thread::yield_to();
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    }

    unsafe {
        thread::end_yield();
    }

    // Spawn a new thread every second in a loop
    // std::thread::spawn(|| {
    //     let mut count = 0;
    //     loop {
    //         count += 1;
    //         parfum::thread::spawn_fn(move || {
    //             println!("[dynamic thread] spawned #{}", count);
    //             for j in 0..3 {
    //                 println!("[dynamic thread #{}] j = {}", count, j);
    //                 std::thread::sleep(Duration::from_millis(300));
    //             }
    //             println!("[dynamic thread #{}] done!", count);
    //         });
    //         std::thread::sleep(Duration::from_secs(1));
    //     }
    // });

    // Start the thread pool as before
    // thread::start_thread_pool();
}
