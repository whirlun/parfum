use std::{ptr};
use std::sync::atomic::{AtomicPtr, Ordering};

use libc::{mmap, mprotect, MAP_ANON, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE};
use libc::{sigaction, sighandler_t, sigemptyset, sigaltstack, stack_t, SIGSTKSZ, SA_ONSTACK, SA_SIGINFO, SIGSEGV, SA_RESTART};

const PAGE_SIZE: usize = 16384;
const INITIAL_PAGE_COUNT: usize = 4;
const MAX_PAGE: usize = 8 * 1024 * 1024 / PAGE_SIZE;

#[repr(C)]
pub struct StackRegion {
    pub start: *mut u8,
    pub end: *mut u8,
    pub next: *mut StackRegion,
}

static STACK_REGISTRY_HEAD: AtomicPtr<StackRegion> = AtomicPtr::new(ptr::null_mut());

pub fn register_stack_region(start: *mut u8, end: *mut u8) {
    let region = Box::into_raw(Box::new(StackRegion {
        start,
        end,
        next: ptr::null_mut(),
    }));
    loop {
        let head = STACK_REGISTRY_HEAD.load(Ordering::Acquire);
        unsafe { (*region).next = head; }
        if STACK_REGISTRY_HEAD
            .compare_exchange(head, region, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break;
        }
    }
}

/// Find the stack region containing the given address (for segv handler)
pub fn find_stack_region(addr: *mut u8) -> Option<&'static StackRegion> {
    let mut current = STACK_REGISTRY_HEAD.load(Ordering::Acquire);
    while !current.is_null() {
        unsafe {
            if addr >= (*current).start && addr < (*current).end {
                return Some(&*current);
            }
            current = (*current).next;
        }
    }
    None
}

pub unsafe fn reserve_stack() -> (usize, usize) {
    let max_size = MAX_PAGE * PAGE_SIZE as usize;
    let region = mmap(ptr::null_mut(), max_size, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if region == libc::MAP_FAILED {
        panic!("Failed to allocate stack memory");
    }

    let top = region as usize + max_size;
    let bottom = top - INITIAL_PAGE_COUNT * PAGE_SIZE;
    let unprot_addr = bottom;
    let ret = mprotect(unprot_addr as *mut _,  INITIAL_PAGE_COUNT * PAGE_SIZE, PROT_READ | PROT_WRITE);
    if ret != 0 {
        println!("Failed to set stack memory protection: {}", std::io::Error::last_os_error());
        panic!("Failed to set stack memory protection");
    }

    register_stack_region(region as *mut u8, (region as usize + max_size) as *mut u8);

    (top, bottom)
}

extern "C" fn segv_hander(_signum: i32, siginfo: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    unsafe {
        let fault_addr = (*siginfo).si_addr as usize;
        {
            use crate::thread::async_signal_safe_print;
            let mut buf = [0u8; 64];
            let msg = b"[SIGSEGV] Fault addr: 0x";
            let mut n = 0;
            // Write prefix
            unsafe { libc::write(libc::STDOUT_FILENO, msg.as_ptr() as *const _, msg.len()); }
            // Write hex address
            let mut addr = fault_addr;
            let mut started = false;
            for i in (0..16).rev() {
                let digit = ((addr >> (i*4)) & 0xf) as u8;
                if digit != 0 || started || i == 0 {
                    started = true;
                    buf[n] = if digit < 10 { b'0' + digit } else { b'a' + (digit-10) };
                    n += 1;
                }
            }
            unsafe { libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const _, n); }
            let nl = b"\n";
            unsafe { libc::write(libc::STDOUT_FILENO, nl.as_ptr() as *const _, 1); }
        }
        if let Some(region) = find_stack_region(fault_addr as *mut u8) {
            let stack_start = region.start as usize;
            let stack_end = region.end as usize;
            let page_aligned_fault = fault_addr & !(PAGE_SIZE - 1);
            if page_aligned_fault >= stack_start && page_aligned_fault < stack_end {
                crate::thread::async_signal_safe_print("[SIGSEGV] Growing stack page\n");
                let ret = mprotect(page_aligned_fault as *mut _, PAGE_SIZE, PROT_READ | PROT_WRITE);
                if ret == 0 {
                    return; 
                }
            }
        }
        libc::_exit(0xff);
    }
}

pub fn install_segv_handler() {
    unsafe {
        let sig_stack_size = SIGSTKSZ as usize;
        let sig_stack = mmap(
            ptr::null_mut(),
            sig_stack_size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        );
        if sig_stack == libc::MAP_FAILED {
            panic!("failed to allocate alt signal stack");
        }
        let ss = stack_t {
            ss_sp: sig_stack,
            ss_size: sig_stack_size,
            ss_flags: 0,
        };
        if sigaltstack(&ss, ptr::null_mut()) != 0 {
            panic!("sigaltstack failed: {}", std::io::Error::last_os_error());
        }

        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = segv_hander as usize;
        sa.sa_flags = SA_SIGINFO | SA_RESTART | SA_ONSTACK;
        sigemptyset(&mut sa.sa_mask);
        if sigaction(SIGSEGV, &sa, ptr::null_mut()) != 0 {
            panic!("Failed to install SIGSEGV handler");
        }
        if sigaction(libc::SIGBUS, &sa, ptr::null_mut()) != 0 {
            panic!("Failed to install SIGBUS handler");
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    // Recursive function to test stack growth
    fn recursive(n: usize) -> usize {
        if n == 0 { 1 } else { 1 + recursive(n - 1) }
    }

    #[test]
    fn test_stack_growth() {
        install_segv_handler();
        unsafe {
            let (_top, _bottom) = reserve_stack();
        }
        // Try to recurse deeply (should trigger stack growth)
        let depth = 2000; // Should be enough to trigger stack growth
        let result = recursive(depth);
        assert_eq!(result, depth + 1);
    }
}