# Parfum — Green threads for Rust on Apple Silicon (arm64)

Parfum is a small, experimental user‑space runtime that runs lightweight “green threads” on macOS (arm64). It combines a cooperative scheduler with timer‑based preemption hints, grow‑as‑you‑go stacks, and a proc‑macro that inserts preemption checks for you.

Status: experimental/research. Expect rough edges and platform assumptions. Not production‑ready.

## Features
- Cooperative scheduler with periodic preemption hints (SIGALRM + `check_preemption`).
- AArch64 context switching in assembly.
- On‑demand stack growth via `mmap`/`mprotect` and a SIGSEGV handler on an alternate signal stack.
- Attribute macro `#[preemptive]` that injects `check_preemption()` before call sites.
- Simple mailbox messaging between green threads: `send` and `try_recv`.

## Requirements
- Rust nightly toolchain (uses `#![feature(generic_atomic)]`).
- macOS on Apple Silicon (arm64).

## Project layout
- `parfum/` — core runtime (scheduler, stack management, context switch glue).
- `parfum-macro/` — proc‑macro crate providing `#[preemptive]`.
- `parfum-example/` — small demo showing preemption and messaging.

## Quick start
- Build everything:
  - `cargo build`
- Run the demo:
  - `cargo run -p parfum-example`
- Run tests for the core crate (includes a stack‑growth test that uses signals):
  - `cargo test -p parfum`

## How it works (overview)
- Scheduling: global injector + thread‑local worker queues (Crossbeam). `yield_to()` hands control back so the next runnable thread can run. A periodic `SIGALRM` sets a flag that `check_preemption()` consumes inside instrumented code to trigger a yield.
- Context switch: `switch(current, next)` in `arch/arm64/switch.S` saves/restores AArch64 registers.
- Stacks: `reserve_stack()` maps a large PROT_NONE region and enables pages on demand in the SIGSEGV handler, which runs on an alternate signal stack.
- Macro: `#[preemptive]` walks the function body and inserts `check_preemption()` before each call (except calls to `check_preemption` itself).
- Messaging: each `GreenThread` has a mutex‑guarded mailbox; `send` enqueues messages and `try_recv::<T>()` pulls the first matching type.

## Example
See `parfum-example/src/main.rs` for a compact program that sets up preemption, spawns green threads, and passes messages.

## Limitations
- arm64/macOS only; no x86_64 path.
- Blocking syscalls can stall the scheduler unless you yield around them.
- Signal‑based preemption is best‑effort; uninstrumented code won’t be preempted.
- Heavy use of `unsafe` and signals; undefined behavior is possible.
