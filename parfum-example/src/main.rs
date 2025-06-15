use parfum::thread;
use parfum::stack::install_segv_handler;
use std::time::Duration;
use parfum_macro::preemptive;
use crate::thread::check_preemption;


fn hello() -> String {
    "Hello, world!".to_string()
}
#[preemptive]
fn send_message() {
    let tid = thread::spawn_fn(|| {
        loop {
            if let Some((_from, msg)) = thread::try_recv::<String>() {
                println!("Received: {}", msg);
            }
            thread::yield_to();
        }
    });

    for i in 0..20 {
        let hello = hello();
        let msg = format!("hello {}", i);
        thread::send(tid, msg).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(2000));
    thread::spawn_fn(|| {
        println!("Exiting sender thread");
    });
    thread::yield_to();
}

#[preemptive]
fn print_numbers(label: &'static str, count: usize) {
    for i in 0..count {
        println!("{}: {}", label, i);
        thread::yield_to();
    }
}

#[preemptive]
fn multi_thread_demo() {
    let _t1 = thread::spawn_fn(|| print_numbers("Even", 5));
    let _t2 = thread::spawn_fn(|| print_numbers("Odd", 5));
    // let both coroutines run
    unsafe { thread::end_yield(); }
}

fn main() {
    install_segv_handler();
    parfum::thread::setup_preemption();
    thread::spawn_fn(|| send_message());
    thread::spawn_fn(|| multi_thread_demo());
    unsafe {
        thread::end_yield();
    }
}
