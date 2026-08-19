#include "wave1.h"

int main(void)
{
    if (sys_getuid() != 0 || sys_getgid() != 0) {
        tfail("getuid/getgid are not 0");
    } else {
        tpass("getuid/getgid");
    }
    sys_exit(wave1_done());
}
