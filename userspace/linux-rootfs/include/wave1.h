#ifndef WAVE1_H
#define WAVE1_H

#include "linux_sys.h"

#define TPASS 0
#define TFAIL 1

extern int fail_count;

void tpass(const char *msg);
void tfail(const char *msg);
int wave1_done(void);

#endif
