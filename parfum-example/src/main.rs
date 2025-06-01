use parfum::thread;
use parfum::stack::install_segv_handler;
use std::time::Duration;

fn main() {
    install_segv_handler();
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
    

    // Test stack growth with a deep recursive function
    fn deep_rec(n: usize) -> usize {
        println!("[deep_rec] n = {}", n);
        if n == 0 { 1 } else { 1 + deep_rec(n - 1) }
    }
    // let depth = 2000; // Should be enough to trigger stack growth
    // let result = deep_rec(depth);
    // println!("[main] deep_rec({}) = {}", depth, result);
    // assert_eq!(result, depth + 1);

    parfum::thread::spawn_fn(|| {
        deep_rec(1000);
    });

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
