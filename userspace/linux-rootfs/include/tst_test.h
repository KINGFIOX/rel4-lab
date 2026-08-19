#ifndef TST_TEST_H
#define TST_TEST_H

#include "linux_sys.h"

#define TPASS 0
#define TFAIL 1
#define TINFO 2

extern int TST_PASS;
extern int tst_failed;

void tst_res(int type, const char *fmt, ...);

#define TST_EXP_PASS(call)                                                     \
    do {                                                                       \
        long _ret = (long)(call);                                              \
        if (_ret == 0) {                                                       \
            TST_PASS = 1;                                                      \
            tst_res(TPASS, #call " succeeded");                                \
        } else {                                                               \
            TST_PASS = 0;                                                      \
            tst_failed = 1;                                                    \
            tst_res(TFAIL, #call " failed");                                   \
        }                                                                      \
    } while (0)

struct tst_test {
    void (*test_all)(void);
};

#endif
