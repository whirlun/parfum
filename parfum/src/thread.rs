use core::arch::{global_asm, asm};
use std::cell::{RefCell, UnsafeCell};
use std::include_str;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use crossbeam::deque::{Injector, Worker};
use once_cell::sync::Lazy;
use std::thread;

use crate::stack::reserve_stack;

static PREEMPTION_REQUESTED: AtomicBool = AtomicBool::new(false);

static COOP_INJECTOR: Lazy<Injector<Arc<GreenThread>>> = Lazy::new(Injector::new);

static SCHEDULER: Lazy<Mutex<Scheduler>> = Lazy::new(|| Mutex::new(Scheduler::new()));

thread_local! {
    static COOP_WORKER: Worker<Arc<GreenThread>> = Worker::new_fifo();
    static PREEPMT_WORKER: Worker<Arc<GreenThread>> = Worker::new_fifo();
    static CURRENT: RefCell<Option<Arc<GreenThread>>> = RefCell::new(None);
}

#[derive(Debug)]
pub struct GreenThread {
    pub context: UnsafeCell<ThreadContext>,
    pub stack_top: usize,
    pub stack_bottom: AtomicUsize,
    pub tid: usize,
    pub state: AtomicUsize,
    pub ticks: AtomicI32,
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
    YIELD = 2,
    READY = 3,
    EXITED = 4,
}

pub struct Scheduler {
    threads: Mutex<Vec<Arc<GreenThread>>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            threads: Mutex::new(Vec::new()),
        }
    }

    pub fn spawn_fn<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let wrapper = Box::new(ClosureWrapper { closure: Some(Box::new(f)) });
        let wrapper_ptr = Box::into_raw(wrapper) as *mut ();
        println!("[spawn_fn] wrapper_ptr: {:p}", wrapper_ptr);
        let mut ctx = ThreadContext::zeroed();
        let (top, bottom) = unsafe { reserve_stack() };
        init_stack_with_arg(top, &mut ctx, rust_trampoline, wrapper_ptr);
        let tid = self.threads.lock().unwrap().len();
        let thread = Arc::new(GreenThread {
            context: UnsafeCell::new(ctx),
            stack_top: top,
            stack_bottom: AtomicUsize::new(bottom),
            tid,
            state: AtomicUsize::new(ThreadState::READY as usize),
            ticks: AtomicI32::new(100),
            next: AtomicPtr::new(std::ptr::null_mut()),
        });
        println!("Spawning thread {} (closure)", tid);
        self.threads.lock().unwrap().push(thread.clone());
        COOP_INJECTOR.push(thread);
    }
}

pub fn init_stack(stack_top: usize, ctx: &mut ThreadContext, entry: unsafe extern "C" fn()) {
    let sp = stack_top & !0x0F;
    assert!((sp as usize) >= stack_top);

    ctx.sp = sp;
    ctx.regs = [0; 32];
    ctx.regs[30] = end_yield as usize;
    ctx.fpregs = [0; 32];
    ctx.pc = entry as usize;
}

pub fn init_stack_with_arg(
    stack_top: usize,
    ctx: &mut ThreadContext,
    entry: unsafe extern "C" fn(*mut ()),
    arg: *mut (),
) {
    let sp = stack_top & !0x0F;
    ctx.sp = sp;
    ctx.regs = [0; 32];
    ctx.regs[0] = arg as usize;
    ctx.regs[30] = end_yield as usize;
    ctx.fpregs = [0; 32];
    ctx.pc = entry as usize;
}

fn setup_timer() {
    unsafe {
        let timer = libc::itimerval {
            it_interval: libc::timeval {
                tv_sec: 0,
                tv_usec: 10000, // 50ms interval
            },
            it_value: libc::timeval {
                tv_sec: 0,
                tv_usec: 10000, // 50ms initial delay
            },
        };
        if libc::setitimer(libc::ITIMER_REAL, &timer, std::ptr::null_mut()) != 0 {
            panic!("Failed to set timer");
        }
        println!("Timer set up for preemption (50ms interval)");
    }
}

extern "C" fn sigalrm_handler(_signum: i32, _info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    PREEMPTION_REQUESTED.store(true, Ordering::Release);
    
    // async_signal_safe_print("[SIGNAL] SIGALRM received - preemption requested\n");
}

pub fn setup_preemption() {
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

pub fn check_preemption() -> bool {
    if PREEMPTION_REQUESTED.load(Ordering::Acquire) {
        PREEMPTION_REQUESTED.store(false, Ordering::Release);
        println!("Preemption requested, yielding thread");
        yield_to();
        return true;
    }
    return false;
}


pub fn yield_to() {
    unsafe {
        let cur = CURRENT.with(|c| c.borrow().clone());
        debug_assert!(cur.is_some(), "yield_now with no CURRENT thread");
        
        let cur = cur.unwrap();
        cur.state.store(ThreadState::YIELD as usize, Ordering::Release);
        println!("Yielding thread {}", cur.tid);
        COOP_INJECTOR.push(cur.clone());
        
        loop {
            let next: Option<Arc<GreenThread>> = COOP_WORKER.with(|local| {
                local.pop().or_else(|| {
                    COOP_INJECTOR.steal().success()
                })
            });
            
            if next.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            
            let next = next.unwrap();

            println!("{:?}", next);
            println!("Switching from {} to thread {} cooperatively", cur.tid, next.tid);
            
            next.state.store(ThreadState::RUNNING as usize, Ordering::Release);
            
            CURRENT.with(|c| {
                *c.borrow_mut() = Some(next.clone());
            });
            
            let ctx = cur.context.get();
            let next_ctx = next.context.get();
            
            switch(ctx, next_ctx);
            
            return;
        }
    }
}

pub unsafe extern "C" fn end_yield() {
    let cur = CURRENT.with(|c| c.borrow().clone());
    let mut cur_tid = -1;
    
    if let Some(cur) = cur {
        if cur.state.load(Ordering::Acquire) == ThreadState::RUNNING as usize {
            cur.state.store(ThreadState::EXITED as usize, Ordering::Release);
        }
        cur_tid = cur.tid as i32;
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
        println!("Switching from {} to thread {} after exited", cur_tid, next.tid);
        
        next.state.store(ThreadState::RUNNING as usize, Ordering::Release);
        
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(next.clone());
        });
        
        let dummy_ctx = &mut ThreadContext::zeroed();
        let next_ctx = next.context.get();
        
        switch(dummy_ctx, next_ctx);
    }
}

#[no_mangle]
pub extern "C" fn thread_exit() -> ! {
    println!("thread_exit");
    std::process::exit(0);
}

struct ClosureWrapper {
    closure: Option<Box<dyn FnOnce()>>,
}

impl ClosureWrapper {
    fn call(mut self: Box<Self>) {
        if let Some(closure) = self.closure.take() {
            closure();
        }
    }
}

extern "C" fn rust_trampoline(f: *mut ()) {
    println!("[trampoline] received pointer: {:p}", f);
    unsafe {
        let wrapper: Box<ClosureWrapper> = Box::from_raw(f as *mut ClosureWrapper);
        println!("[trampoline] Box::from_raw OK");
        wrapper.call();
        println!("[trampoline] closure called");
        end_yield();
    }
}

pub fn spawn_fn<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    SCHEDULER.lock().unwrap().spawn_fn(f);
}

global_asm!(include_str!("arch/arm64/switch.S"));

extern "C" {
    pub fn switch(current: *mut ThreadContext, next: *const ThreadContext);
}

fn async_signal_safe_print(msg: &str) {
    unsafe {
        let bytes = msg.as_bytes();
        libc::write(libc::STDOUT_FILENO, bytes.as_ptr() as *const libc::c_void, bytes.len());
    }
}