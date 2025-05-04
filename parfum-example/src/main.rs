use parfum::thread;


fn main() {
    unsafe {
        thread::start_thread_pool();
    }
}
