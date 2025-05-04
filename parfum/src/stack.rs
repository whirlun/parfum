use std::{ptr};

use libc::{mmap, mprotect, MAP_ANON, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE};

use crate::thread::THREADS;


const PAGE_SIZE: usize = 16384;
const INITIAL_PAGE_COUNT: usize = 1;
const MAX_PAGE: usize = 8 * 1024 * 1024 / PAGE_SIZE;

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

    (top, bottom)
    
}

extern "C" fn segv_hander(_signum: i32, siginfo: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    unsafe {
        let fault_addr = (*siginfo).si_addr as usize;
        
    }

}