#include "wave1.h"

int fail_count;

void tpass(const char *msg)
{
    prints("TPASS ");
    prints(msg);
    prints("\n");
}

void tfail(const char *msg)
{
    prints("TFAIL ");
    prints(msg);
    prints("\n");
    fail_count++;
}

int wave1_done(void)
{
    return fail_count == 0 ? 0 : 1;
}
