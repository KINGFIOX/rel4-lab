/*
 * Copyright 2017, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include <autoconf.h>
#ifdef CONFIG_ARCH_IA32
#define SP esp
#define STACK_OFFSET_BYTES $12
#else
#define SP rsp
#define STACK_OFFSET_BYTES $8
#endif

.global align_test_asm
align_test_asm:
    /* Align the stack to 16 bytes assuming:
     * - 16 byte stack alignment before this function was called
     * - the register-sized return address has been pushed by the
     *   call and nothing else
     */
  sub STACK_OFFSET_BYTES, %SP

  /* Push and pop the %xmm0 */
  sub $16, %SP

  movdqa %xmm0, (%SP)
  movdqa (%SP), %xmm0

  add $16, %SP

  /* Compensate for stack alignment */
  add STACK_OFFSET_BYTES, %SP

  ret
