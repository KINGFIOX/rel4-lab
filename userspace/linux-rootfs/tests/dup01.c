#include "wave1.h"

int main(void)
{
    int fd = sys_dup(1);
    if (fd < 0) {
        tfail("dup stdout failed");
        sys_exit(1);
    }
    const char msg[] = "dup01\n";
    if (sys_write(fd, msg, sizeof(msg) - 1) != (ssize_t)(sizeof(msg) - 1)) {
        tfail("write on dup fd failed");
    }
    int fd3 = sys_dup3(1, 8, 0);
    if (fd3 != 8) {
        tfail("dup3 did not use the requested fd");
    } else {
        tpass("dup/dup3");
    }
    sys_close(fd);
    if (fd3 >= 0) {
        sys_close(fd3);
    }
    sys_exit(wave1_done());
}
