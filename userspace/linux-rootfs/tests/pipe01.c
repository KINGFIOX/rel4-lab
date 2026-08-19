#include "wave1.h"

int main(void)
{
    int fds[2];
    fds[0] = -1;
    fds[1] = -1;
    if (sys_pipe2(fds, 0) != 0) {
        tfail("pipe2 failed");
        sys_exit(1);
    }
    const char msg[] = "xy";
    if (sys_write(fds[1], msg, 2) != 2) {
        tfail("pipe write failed");
    }
    char buf[4];
    ssize_t n = sys_read(fds[0], buf, sizeof(buf));
    if (n != 2 || buf[0] != 'x' || buf[1] != 'y') {
        tfail("pipe read mismatch");
    } else {
        tpass("pipe2");
    }
    sys_close(fds[0]);
    sys_close(fds[1]);
    sys_exit(wave1_done());
}
