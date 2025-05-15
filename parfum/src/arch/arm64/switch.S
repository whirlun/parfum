.global _switch
.global _recover

_switch:
    mov x9, sp
    str x9, [x0, #0]

    str x19, [x0, #0xa8]
    stp x20, x21, [x0, #0xb0]
    stp x22, x23, [x0, #0xc0]
    stp x24, x25, [x0, #0xd0]
    stp x26, x27, [x0, #0xe0]
    stp x28, x29, [x0, #0xf0]
    str x30, [x0, #0x100]

    stp q0, q1, [x0, #0x110]
    stp q2, q3, [x0, #0x130]
    stp q4, q5, [x0, #0x150]
    stp q6, q7, [x0, #0x170]
    stp q8, q9, [x0, #0x190]
    stp q10, q11, [x0, #0x1b0]
    stp q12, q13, [x0, #0x1d0]
    stp q14, q15, [x0, #0x1f0]
    stp q16, q17, [x0, #0x210]
    stp q18, q19, [x0, #0x230]
    stp q20, q21, [x0, #0x250]
    stp q22, q23, [x0, #0x270]
    stp q24, q25, [x0, #0x290]
    stp q26, q27, [x0, #0x2b0]
    stp q28, q29, [x0, #0x2d0]
    stp q30, q31, [x0, #0x2f0]

    str lr, [x0, #0x310]

    dmb ish

    ldr x9, [x1, #0]
    mov sp, x9

    ldr x19, [x1, #0xa8]
    ldp x20, x21, [x1, #0xb0]
    ldp x22, x23, [x1, #0xc0]
    ldp x24, x25, [x1, #0xd0]
    ldp x26, x27, [x1, #0xe0]
    ldp x28, x29, [x1, #0xf0]
    ldr x30, [x1, #0x100]
    ldp q0, q1, [x1, #0x110]
    ldp q2, q3, [x1, #0x130]
    ldp q4, q5, [x1, #0x150]
    ldp q6, q7, [x1, #0x170]
    ldp q8, q9, [x1, #0x190]
    ldp q10, q11, [x1, #0x1b0]
    ldp q12, q13, [x1, #0x1d0]
    ldp q14, q15, [x1, #0x1f0]
    ldp q16, q17, [x1, #0x210]
    ldp q18, q19, [x1, #0x230]
    ldp q20, q21, [x1, #0x250]
    ldp q22, q23, [x1, #0x270]
    ldp q24, q25, [x1, #0x290]
    ldp q26, q27, [x1, #0x2b0]
    ldp q28, q29, [x1, #0x2d0]
    ldp q30, q31, [x1, #0x2f0]

    // Restore x0 (argument register) from context
    ldr x0, [x1, #0x10]

    //ldr lr, [x1, #0x310]

    ldr x16, [x1, #0x310]
    br x16