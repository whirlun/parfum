use core::arch::{global_asm, asm};
use std::cell::{RefCell, UnsafeCell};
use std::include_str;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use libc::__darwin_arm_thread_state64 as DarwinArmThreadState;
use libc::__darwin_arm_neon_state64 as DarwinArmNeonState;
use crossbeam::deque::{Injector, Worker};
use once_cell::sync::Lazy;
use std::thread;

use crate::stack::reserve_stack;
static COOP_INJECTOR: Lazy<Injector<Arc<GreenThread>>> = Lazy::new(Injector::new);
static PREEPMT_INJECTOR: Lazy<Injector<Arc<GreenThread>>> = Lazy::new(Injector::new);
pub static THREADS: Mutex<Vec<Arc<GreenThread>>> = Mutex::new(Vec::new());
thread_local! {
    static COOP_WORKER: Worker<Arc<GreenThread>> = Worker::new_fifo();
    static PREEPMT_WORKER: Worker<Arc<GreenThread>> = Worker::new_fifo();
    static CURRENT: RefCell<Option<Arc<GreenThread>>> = RefCell::new(None);
}
pub struct GreenThread {
    pub context: UnsafeCell<ThreadContext>,
    pub stack_top: usize,
    pub stack_bottom: AtomicUsize,
    pub tid: usize,
    pub state: AtomicUsize,
    pub ticks: AtomicI32,
    pub preempt_active: bool,
    pub next: AtomicPtr<GreenThread>,
}

unsafe impl Send for GreenThread {}
unsafe impl Sync for GreenThread {}

#[repr(C)]
#[derive(Default)]
pub struct ThreadContext {
    pub sp: usize,
    pad: usize,
    pub regs: [usize; 32],
    pub fpregs: [u128; 32],
    pub pc: usize,
    pad2: usize,
}


impl ThreadContext {
    pub const fn zeroed() -> Self {
        Self {
            sp: 0,
            pad: 0,
            regs: [0; 32],
            fpregs: [0; 32],
            pc: 0,
            pad2: 0,
        }
    }
}

pub enum ThreadState {
    RUNNING = 0,
    WAITING = 1,
    READY = 2,
    EXITED = 3,
    YIELD = 4,
    PREEMPT = 5
}


pub struct Scheduler {
}

impl Scheduler {
    pub fn new() -> Self {
        Self {}
    }

    pub fn spawn(&mut self, entry: unsafe extern "C" fn()) {
        let mut ctx = ThreadContext::zeroed();
        let (top, bottom) = unsafe { reserve_stack()} ;
        init_stack(top, &mut ctx, true, entry);
        let tid = THREADS.lock().unwrap().len();
        let thread = Arc::new(GreenThread {
            context: UnsafeCell::new(ctx),
            stack_top: top,
            stack_bottom: AtomicUsize::new(bottom),
            tid,
            state: AtomicUsize::new(ThreadState::READY as usize),
            ticks: AtomicI32::new(100),
            preempt_active: true,
            next: AtomicPtr::new(std::ptr::null_mut()),
        });
        println!("Spawning thread {}", tid);
        THREADS.lock().unwrap().push(thread.clone());
        PREEPMT_INJECTOR.push(thread);
    }

    pub fn spawn_yield(&mut self, entry: unsafe extern "C" fn()) {
        let mut ctx = ThreadContext::zeroed();
        let (top, bottom) = unsafe { reserve_stack() };
        init_stack(top, &mut ctx, false, entry);
        let tid = THREADS.lock().unwrap().len();
        let thread = Arc::new(GreenThread {
            context: UnsafeCell::new(ctx),
            stack_top: top,
            stack_bottom: AtomicUsize::new(bottom),
            tid,
            state: AtomicUsize::new(ThreadState::READY as usize),
            ticks: AtomicI32::new(100),
            preempt_active: false,
            next: AtomicPtr::new(std::ptr::null_mut()),
        });
        println!("Spawning thread {}", tid);
        THREADS.lock().unwrap().push(thread.clone());
        COOP_INJECTOR.push(thread);
    }
}


pub fn init_stack(stack_top: usize, ctx: &mut ThreadContext, preempt: bool, entry: unsafe extern "C" fn()) {
    let sp = stack_top & !0x0F; // 16-byte align
    assert!((sp as usize) >= stack_top);

    ctx.sp = sp;
    ctx.regs = [0; 32];
    ctx.regs[30] = if preempt {preempt_end as usize} else {coop_end as usize};
    ctx.fpregs = [0; 32];
    ctx.pc = entry as usize;
    
}

fn setup_timer() {
    unsafe {
        let timer = libc::itimerval {
            it_interval: libc::timeval {
                tv_sec: 0,
                tv_usec: 100000, // 10ms
            },
            it_value: libc::timeval {
                tv_sec: 0,
                tv_usec: 100000, // 10ms
            },
        };
        if libc::setitimer(libc::ITIMER_REAL, &timer, std::ptr::null_mut()) != 0 {
            panic!("Failed to set timer");
        }
        println!("Timer set up for preemption");
    }
}

extern "C" fn sigalrm_handler(_signum: i32, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    unsafe {
        let uc = context as *mut libc::ucontext_t;
        let ss= &mut (*(*uc).uc_mcontext).__ss as &mut DarwinArmThreadState;
        let neon: &mut DarwinArmNeonState = &mut (*(*uc).uc_mcontext).__ns as &mut DarwinArmNeonState;

        let cur = CURRENT.with(|c| c.borrow().clone());
        if cur.is_some() {
            let cur = cur.unwrap();
            let cur_alive = cur.state.load(Ordering::Acquire) != ThreadState::EXITED as usize;
            if cur_alive {
                let tcx =&mut *cur.context.get();
                tcx.sp = (*ss).__sp as usize;
                tcx.pc = (*ss).__pc as usize;
                tcx.regs[29] = (*ss).__fp as usize;
                tcx.regs[30] = (*ss).__lr as usize;

                core::ptr::copy_nonoverlapping(ss.__x.as_ptr(), tcx.regs.as_mut_ptr() as *mut _, 28);
                // for i in 0..28 {
                //     tcx.regs[i] = (*ss).__x[i] as usize;
                // }
                core::ptr::copy_nonoverlapping(neon.__v.as_ptr(), tcx.fpregs.as_mut_ptr() as *mut _, 32);
                
                (*cur).state.store(ThreadState::PREEMPT as usize, Ordering::Release);
                PREEPMT_INJECTOR.push(cur.clone());
            }
        }
        let next = PREEPMT_WORKER.with(|local| {
            local.pop().or_else(|| {
                PREEPMT_INJECTOR.steal().success()
            })
        });
        if next.is_none() {
            return;
        }
        let next = next.unwrap();
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(next.clone());
        });
        (*next).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        let ncx = &*(*next).context.get();
        (*ss).__sp = ncx.sp as u64;
        (*ss).__pc = ncx.pc as u64;
        (*ss).__fp = ncx.regs[29] as u64;
        (*ss).__lr = ncx.regs[30] as u64;
        core::ptr::copy_nonoverlapping(ncx.regs.as_ptr(), ss.__x.as_mut_ptr() as *mut _, 28);
        // for i in 0..28 {
        //     (*ss).__x[i] = ncx.regs[i] as u64;
        // }
        core::ptr::copy_nonoverlapping(ncx.fpregs.as_ptr() as *const u128, neon.__v.as_mut_ptr() as *mut _, 32);
    }
}

fn setup_preemption() {
    unsafe {
        let mut sa: libc::sigaction = libc::sigaction {
            sa_sigaction: sigalrm_handler as usize,
            sa_flags: libc::SA_SIGINFO,
            sa_mask: std::mem::zeroed(),
        };
        if libc::sigemptyset(&mut sa.sa_mask) != 0 {
            panic!("Failed to initialize signal mask");
        }
        if libc::sigaction(libc::SIGALRM, &sa, std::ptr::null_mut()) != 0 {
            panic!("Failed to set signal handler");
        }
    }

    setup_timer();
}

fn test2(s: usize) {
    println!("Test2 function called with value: {}", s);
}

fn test(s: usize) {
    test2(2);
    println!("Test function called with value: {}", s);
}

extern "C" fn hello() {
    test(1);
    //yield_to();
    std::thread::sleep(std::time::Duration::from_millis(2000));
    //yield_to();
    println!("Hello, world!");
}

extern "C" fn world() {
    std::thread::sleep(std::time::Duration::from_millis(2000));
    println!("World, world!");
}

pub fn start_thread_pool() {
    let sched = Arc::new(Mutex::new(Scheduler::new()));
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn(hello);
    sched.lock().unwrap().spawn(world);
    sched.lock().unwrap().spawn_yield(hello);
    sched.lock().unwrap().spawn_yield(world);
    let coop_handle = thread::Builder::new()
        .name("coop_worker".to_string())
        .spawn(move || {
            unsafe {
                let mut mask: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut mask);
                libc::sigaddset(&mut mask, libc::SIGALRM);
                libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
                coop_end();
            }
        })
        .expect("Failed to spawn cooperative worker thread");

    let mut preemptive_handles = Vec::new();
    for i in 0..4 {
        let handle = thread::Builder::new()
            .name(format!("preemptive_worker_{}", i))
            .spawn({
                move || {
                    unsafe {
                        let mut mask: libc::sigset_t = std::mem::zeroed();
                        libc::sigemptyset(&mut mask);
                        libc::sigaddset(&mut mask, libc::SIGALRM);
                        libc::pthread_sigmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut());
                        setup_preemption();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
            })
            .expect("Failed to spawn preemptive worker thread");
        preemptive_handles.push(handle);
    }
    for handle in preemptive_handles {
        handle.join().expect("Failed to join preemptive worker thread");
    }
    coop_handle.join().expect("Failed to join cooperative worker thread");
}

fn yield_to() {
    unsafe {
        let cur = CURRENT.with(|c| c.borrow().clone());
        debug_assert!(!cur.is_none(), "yield_now with no CURRENT thread");
        let cur = cur.unwrap();
        if cur.preempt_active {
            panic!("Cannot yield from a preemptive thread");
        }
        cur.state.store(ThreadState::YIELD as usize, Ordering::Release);
        COOP_INJECTOR.push(cur.clone());
        let next = COOP_WORKER.with(|local| {
            local.pop().or_else(|| {
                COOP_INJECTOR.steal().success()
            })
        });
        let next = next.unwrap();
        println!("Switching to thread {} cooperatively", (*next).tid);
        (*next).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(next.clone());
        });
        let ctx = cur.context.get();
        let next_ctx = (*next).context.get();
        switch(ctx, next_ctx);
    }
}

pub unsafe extern "C" fn preempt_end() {
    let cur = CURRENT.with(|c| c.borrow().clone());
    if let Some(cur) = cur {
        if cur.state.load(Ordering::Acquire) == ThreadState::RUNNING as usize {
            cur.state.store(ThreadState::EXITED as usize, Ordering::Release);
        }
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    
}

pub unsafe extern "C" fn coop_end() {
    let cur = CURRENT.with(|c| c.borrow().clone());
    if let Some(cur) = cur .as_ref(){
        if cur.state.load(Ordering::Acquire) == ThreadState::RUNNING as usize {
            cur.state.store(ThreadState::EXITED as usize, Ordering::Release);
        }
    }
    loop {
        let next = COOP_WORKER.with(|local| {
            local.pop().or_else(|| {
                COOP_INJECTOR.steal().success()
            })
        });
        if next.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            continue;
        }
        let next = next.unwrap();
        println!("Switching to thread {} after exited", (*next).tid);
        (*next).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(next.clone());
        });
        let dummy_ctx = &mut ThreadContext::zeroed();
        let next_ctx = (*next).context.get();
        switch(dummy_ctx, next_ctx);
    }
}

#[no_mangle]
pub extern "C" fn thread_exit() -> ! {
    println!("thread_exit");
    std::process::exit(0);
}

global_asm!(include_str!("arch/arm64/switch.S"));

#[no_mangle]
extern "C" fn trampoline() {
    unsafe {
        // start the actual rust function here
        let sp: usize;
        asm!("mov {}, sp", out(reg) sp);
        let func_ptr = *(sp as *const extern "C" fn());
        func_ptr();
    }
}

extern "C" {
    pub fn switch(current: *mut ThreadContext, next: *const ThreadContext);
}