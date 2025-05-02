use core::arch::{global_asm, asm, naked_asm};
use std::include_str;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};
use crate::scheduler;
use crate::msqueue::MsQueue;
use libc::__darwin_arm_thread_state64 as DarwinArmThreadState;
use libc::__darwin_arm_neon_state64 as DarwinArmNeonState;
use lazy_static::lazy_static;
static mut CTX_SCHEDULER: ThreadContext = ThreadContext::zeroed();
static mut SCHEDULER: *mut Scheduler = std::ptr::null_mut();
static mut CURRENT: AtomicPtr<GreenThread> = AtomicPtr::new(std::ptr::null_mut());
lazy_static! {
    static ref QUEUE: MsQueue = MsQueue::new();
}
pub struct GreenThread {
    pub context: ThreadContext,
    pub stack: Vec<u8>,
    pub tid: usize,
    pub state: AtomicUsize,
    pub ticks: AtomicI32,
    pub next: AtomicPtr<GreenThread>,
}


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
    pub threads: Vec<GreenThread>,
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

    fn next_ready(&self) -> Option<(usize, &GreenThread)> {
        self.threads
        .iter()
        .enumerate()
        .cycle()
        .skip(self.current_thread.load(Ordering::Acquire) + 1)
        .take(self.threads.len())
        .find(|(_, thread)| {
            thread.state.load(Ordering::Acquire) == ThreadState::READY as usize
        })
    }

    pub fn spawn(&mut self, entry: unsafe extern "C" fn()) {
        let mut ctx = ThreadContext::zeroed();
        let mut stack = vec![0u8; 4096];
        init_stack(&mut stack, &mut ctx, entry);
        let tid = self.threads.len();
        let thread = GreenThread {
            context: ctx,
            stack,
            tid,
            state: AtomicUsize::new(ThreadState::READY as usize),
            ticks: AtomicI32::new(100),
            next: AtomicPtr::new(std::ptr::null_mut()),
        };
        println!("Spawning thread {}", tid);
        self.threads.push(thread);
    }
}


pub fn init_stack(stack: &mut [u8], ctx: &mut ThreadContext, entry: unsafe extern "C" fn()) {
    let sp = (stack.as_mut_ptr() as usize + stack.len()) & !0x0F; // 16-byte align
    assert!((sp as usize) >= stack.as_ptr() as usize);
    unsafe {
        // let stack_top = (sp - 32) as *mut usize;
        // *stack_top = 0;
        //*(stack_top.add(1)) = event_loop_entry as usize;
        //*(stack_top.add(1)) = entry as usize;

        ctx.sp = sp;
        ctx.regs = [0; 32];
        ctx.regs[30] = event_loop_entry as usize;
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

        let cur = CURRENT.load(Ordering::Relaxed);
        if cur.is_null() {
            return;
        }

        let tcx = &mut (*cur).context;
        tcx.sp = (*ss).__sp as usize;
        tcx.pc = (*ss).__pc as usize;
        tcx.regs[29] = (*ss).__fp as usize;
        tcx.regs[30] = (*ss).__lr as usize;

        core::ptr::copy_nonoverlapping(ss.__x.as_ptr(), tcx.regs.as_mut_ptr() as *mut _, 28);
        // for i in 0..28 {
        //     tcx.regs[i] = (*ss).__x[i] as usize;
        // }
        core::ptr::copy_nonoverlapping(neon.__v.as_ptr(), tcx.fpregs.as_mut_ptr() as *mut _, 32);
        
        (*cur).state.store(ThreadState::READY as usize, Ordering::Release);
        let mut next = QUEUE.dequeue();

        if next.is_null() {
            next = cur;
        } else {
            QUEUE.enqueue(cur);
        }

        //println!("Switching to thread {}", (*next).tid);
        CURRENT.store(next, Ordering::Release);
        (*next).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        let ncx = &mut (*next).context;
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
    yield_to();
    println!("Hello, world!");
}

extern "C" fn world() {
    //std::thread::sleep(std::time::Duration::from_millis(2000));
    println!("World, world!");
}

pub fn run() {
    unsafe {
        SCHEDULER = Box::into_raw(Box::new(Scheduler::new()));
        let scheduler = &mut *SCHEDULER;
        scheduler.spawn(hello);
        scheduler.spawn(world);
        for t in scheduler.threads.iter().skip(1) {
            QUEUE.enqueue(t as *const _ as *mut _);
        }
        let first = scheduler.threads.first().unwrap() as *const _ as *mut _;
        CURRENT.store(first, Ordering::Release);
        
       //setup_preemption();
       (*first).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        let mut dummy_ctx = ThreadContext::zeroed();
        switch(&mut dummy_ctx, &(*first).context);
        core::hint::unreachable_unchecked();
    }

}

fn yield_to() {
    unsafe {
        let cur = CURRENT.load(Ordering::Relaxed);
        debug_assert!(!cur.is_null(), "yield_now with no CURRENT thread");
        (*cur).state.store(ThreadState::YIELD as usize, Ordering::Release);
        QUEUE.enqueue(cur);
        let next = QUEUE.dequeue();
        if next.is_null() {
            println!("No more threads to run, exiting.");
            thread_exit();
        }
        println!("Switching to thread {}", (*next).tid);
        (*next).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        CURRENT.store(next, Ordering::Release);
        let ctx = &mut (*cur).context;
        switch(ctx, &(*next).context);
    }
}

pub unsafe extern "C" fn event_loop_entry() {
    loop {
        let cur = CURRENT.load(Ordering::Relaxed);
        let ctx = &mut (*cur).context;
        if let Some(cur) = CURRENT.load(Ordering::Relaxed).as_mut() {
            if cur.state.load(Ordering::Acquire) == ThreadState::READY as usize || cur.state.load(Ordering::Acquire) == ThreadState::YIELD as usize {
                QUEUE.enqueue(cur);
            } else if cur.state.load(Ordering::Acquire) == ThreadState::RUNNING as usize {
                cur.state.store(ThreadState::EXITED as usize, Ordering::Release);
            }
        }
        let next = QUEUE.dequeue();
        if next.is_null() {
            println!("No more threads to run, exiting.");
            thread_exit();
        }
        println!("Switching to thread {}", (*next).tid);
        (*next).state.store(ThreadState::RUNNING as usize, Ordering::Release);
        CURRENT.store(next, Ordering::Release);
        switch(ctx, &(*next).context);
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