#include "tst_test.h"

#ifndef LTP_TEST_C
#error LTP_TEST_C must name an LTP test source
#endif

#include LTP_TEST_C

int TST_PASS;
int tst_failed;
int errno;

void tst_res(int type, const char *fmt, ...)
{
    if (type == TPASS) {
        prints("TPASS ");
    } else if (type == TFAIL) {
        prints("TFAIL ");
        tst_failed = 1;
    } else {
        prints("TINFO ");
    }
    prints(fmt);
    prints("\n");
}

int main(void)
{
    if (test.test_all != 0) {
        test.test_all();
    }
    return tst_failed ? 1 : 0;
}
