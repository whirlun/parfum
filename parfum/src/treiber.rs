use core::sync::atomic::{AtomicUsize, AtomicPtr, Ordering};
use crate::thread::GreenThread;
pub struct TreiberStack {
    head: AtomicPtr<GreenThread>,
}

impl TreiberStack {
    pub const fn new() -> Self {
        TreiberStack {
            head: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    pub fn push(&self, t: *mut GreenThread) {
        unsafe {
            loop {
                let head = self.head.load(Ordering::Acquire);
                (*t).context.regs[31] = head as usize; 
                if self.head.compare_exchange(head, t, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    break;
                }
            }
        }
    }

    pub fn pop(&self) -> *mut GreenThread {
        unsafe {
            loop {
                let head = self.head.load(Ordering::Acquire);
                if head.is_null() {
                    return std::ptr::null_mut();
                }
                let next = (*head).context.regs[31] as *mut GreenThread;
                if self.head.compare_exchange(head, next, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    return head;
                }
            }
        }
    }
}