/*
 * Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <elfloader_common.h>

#define UART0_PADDR 0x1fe001e0
#define UART_REG(offset) ((volatile unsigned char *)(UART0_PADDR + (offset)))
#define UART_THR 0x00
#define UART_LSR 0x05
#define UART_LSR_THRE BIT(5)

int plat_console_putchar(unsigned int c)
{
    while ((*UART_REG(UART_LSR) & UART_LSR_THRE) == 0) {
    }
    *UART_REG(UART_THR) = c & 0xffu;
    return 0;
}
