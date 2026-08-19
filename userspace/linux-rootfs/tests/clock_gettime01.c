#include "wave1.h"

int main(void)
{
    struct timespec ts;
    ts.tv_sec = -1;
    ts.tv_nsec = -1;
    if (sys_clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        tfail("clock_gettime failed");
    } else if (ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1000000000) {
        tfail("clock_gettime produced a bad timespec");
    } else {
        tpass("clock_gettime");
    }
    sys_exit(wave1_done());
}
