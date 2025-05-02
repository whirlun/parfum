use core::sync::atomic::{AtomicPtr, AtomicUsize, AtomicI32, Ordering};

use crate::thread::GreenThread;

pub struct MsQueue {
    dummy: &'static GreenThread,
    head: AtomicPtr<GreenThread>,
    tail: AtomicPtr<GreenThread>,
}

impl MsQueue {
    pub fn new() -> Self {
        let dummy = Box::new(GreenThread {
            context: crate::thread::ThreadContext::zeroed(),
            stack: Vec::new(),
            tid: usize::MAX,
            state: AtomicUsize::new(0),
            ticks: AtomicI32::new(0),
            next: AtomicPtr::new(std::ptr::null_mut()),
        });
        let dummy: &'static GreenThread = Box::leak(dummy);
        let ptr = dummy as *const _ as *mut GreenThread;
        Self {
        dummy,
        head: AtomicPtr::new(ptr),
        tail: AtomicPtr::new(ptr),
        }
    }

    pub fn enqueue(&self, t: *mut GreenThread) {
        unsafe {
            (*t).next.store(std::ptr::null_mut(), Ordering::Relaxed); 
            let prev: *mut GreenThread = self.tail.swap(t, Ordering::AcqRel);
            (*prev).next.store(t, Ordering::Release);
        }
    }

    pub fn dequeue(&self) -> *mut GreenThread {
        unsafe {
            loop {
                let head = self.head.load(Ordering::Acquire);
                let next = (*head).next.load(Ordering::Acquire);
                if next.is_null() {
                    return std::ptr::null_mut();
                }
                if self.head.compare_exchange(head, next, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    return next; 
                }
            }
        }
    }
}