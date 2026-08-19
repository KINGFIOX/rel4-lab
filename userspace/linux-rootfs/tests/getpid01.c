#include "wave1.h"

int main(void)
{
    pid_t pid = sys_getpid();
    pid_t ppid = sys_getppid();
    if (pid <= 0) {
        tfail("getpid returned non-positive");
    } else if (ppid <= 0) {
        tfail("getppid returned non-positive");
    } else {
        tpass("getpid/getppid");
    }
    sys_exit(wave1_done());
}
