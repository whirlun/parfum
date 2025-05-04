use core::arch::{global_asm, asm, naked_asm};
use std::cell::{RefCell, UnsafeCell};
use std::include_str;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use crate::scheduler;
use libc::__darwin_arm_thread_state64 as DarwinArmThreadState;
use libc::__darwin_arm_neon_state64 as DarwinArmNeonState;
use lazy_static::lazy_static;
use crossbeam::deque::{Injector, Steal, Stealer, Worker};
use once_cell::sync::Lazy;
use std::thread;
static mut CTX_SCHEDULER: ThreadContext = ThreadContext::zeroed();
static mut SCHEDULER: *mut Scheduler = std::ptr::null_mut();
static COOP_INJECTOR: Lazy<Injector<Arc<GreenThread>>> = Lazy::new(Injector::new);
static PREEPMT_INJECTOR: Lazy<Injector<Arc<GreenThread>>> = Lazy::new(Injector::new);
static COOP_STEALERS: Lazy<Mutex<Vec<Stealer<Arc<GreenThread>>>>> = Lazy::new(Default::default);
static PREEMT_STEALERS: Lazy<Mutex<Vec<Stealer<Arc<GreenThread>>>>> = Lazy::new(Default::default);
thread_local! {
    static COOP_WORKER: Worker<Arc<GreenThread>> = Worker::new_fifo();
    static PREEPMT_WORKER: Worker<Arc<GreenThread>> = Worker::new_fifo();
    static CURRENT: RefCell<Option<Arc<GreenThread>>> = RefCell::new(None);
}
pub struct GreenThread {
    pub context: UnsafeCell<ThreadContext>,
    pub stack: Vec<u8>,
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
    pub threads: Vec<Arc<GreenThread>>,
    pub current_thread: AtomicUsize,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            threads: Vec::new(),
            current_thread: AtomicUsize::new(0),
        }
    }

    fn mark_thread_state(&self, tid: usize, state: ThreadState) {
        if let Some(thread) = self.threads.get(tid) {
            thread.state.store(state as usize, Ordering::SeqCst);
        }
    }

    pub fn spawn(&mut self, entry: unsafe extern "C" fn()) {
        let mut ctx = ThreadContext::zeroed();
        let mut stack = vec![0u8; 4096];
        init_stack(&mut stack, &mut ctx, true, entry);
        let tid = self.threads.len();
        let thread = Arc::new(GreenThread {
            context: UnsafeCell::new(ctx),
            stack,
            tid,
            state: AtomicUsize::new(ThreadState::READY as usize),
            ticks: AtomicI32::new(100),
            preempt_active: true,
            next: AtomicPtr::new(std::ptr::null_mut()),
        });
        println!("Spawning thread {}", tid);
        self.threads.push(thread.clone());
        PREEPMT_INJECTOR.push(thread);
    }

    pub fn spawn_yield(&mut self, entry: unsafe extern "C" fn()) {
        let mut ctx = ThreadContext::zeroed();
        let mut stack = vec![0u8; 4096];
        init_stack(&mut stack, &mut ctx, false, entry);
        let tid = self.threads.len();
        let thread = Arc::new(GreenThread {
            context: UnsafeCell::new(ctx),
            stack,
            tid,
            state: AtomicUsize::new(ThreadState::READY as usize),
            ticks: AtomicI32::new(100),
            preempt_active: false,
            next: AtomicPtr::new(std::ptr::null_mut()),
        });
        println!("Spawning thread {}", tid);
        self.threads.push(thread.clone());
        COOP_INJECTOR.push(thread);
    }
}


pub fn init_stack(stack: &mut [u8], ctx: &mut ThreadContext, preempt: bool, entry: unsafe extern "C" fn()) {
    let sp = (stack.as_mut_ptr() as usize + stack.len()) & !0x0F; // 16-byte align
    assert!((sp as usize) >= stack.as_ptr() as usize);
    unsafe {
        // let stack_top = (sp - 32) as *mut usize;
        // *stack_top = 0;
        //*(stack_top.add(1)) = event_loop_entry as usize;
        //*(stack_top.add(1)) = entry as usize;

        ctx.sp = sp;
        ctx.regs = [0; 32];
        ctx.regs[30] = if preempt {preempt_end as usize} else {coop_end as usize};
        ctx.fpregs = [0; 32];
        ctx.pc = entry as usize;
    }
}

fn setup_timer() {
    unsafe {
        let timer = libc::itimerval {
            it_interval: libc::timeval {
                tv_sec: 0,
                tv_usec: 10000, // 10ms
            },
            it_value: libc::timeval {
                tv_sec: 0,
                tv_usec: 10000, // 10ms
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
    //std::thread::sleep(std::time::Duration::from_millis(2000));
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

pub fn run() {
    unsafe {
        SCHEDULER = Box::into_raw(Box::new(Scheduler::new()));
        let scheduler = &mut *SCHEDULER;
        //scheduler.spawn_yield(hello);
        //scheduler.spawn_yield(world);
        scheduler.spawn(world);
        scheduler.spawn(hello);
        // let first = scheduler.threads.first().unwrap();
        // CURRENT.with(|c| {
        //     *c.borrow_mut() = Some(Arc::from_raw(first));
        // });
        
       setup_preemption();
       loop{};
    //    (*first).state.store(ThreadState::RUNNING as usize, Ordering::Release);
    //     let mut dummy_ctx = ThreadContext::zeroed();
    //     let ctx = first.context.get();
    //     switch(&mut dummy_ctx, ctx);
    //     core::hint::unreachable_unchecked();
        //coop_end();
    }

}

fn yield_to() {
    unsafe {
        let next = COOP_WORKER.with(|local| {
            local.pop().or_else(|| {
                COOP_INJECTOR.steal().success()
            })
        });
        let cur = CURRENT.with(|c| c.borrow().clone());
        debug_assert!(!cur.is_none(), "yield_now with no CURRENT thread");
        let cur = cur.unwrap();
        if cur.preempt_active {
            panic!("Cannot yield from a preemptive thread");
        }
        cur.state.store(ThreadState::YIELD as usize, Ordering::Release);
        COOP_INJECTOR.push(cur.clone());
        if next.is_none() {
            println!("No more threads to run, exiting.");
            thread_exit();
        }
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
    loop {}
    
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