#include "wave1.h"

int main(void)
{
    const char path[] = "/tmp/open01.txt";
    const char payload[] = "abc";
    int fd = sys_openat(AT_FDCWD, path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        tfail("openat create failed");
        sys_exit(1);
    }
    if (sys_write(fd, payload, 3) != 3) {
        tfail("write file failed");
    }
    if (sys_close(fd) != 0) {
        tfail("close failed");
    }
    fd = sys_openat(AT_FDCWD, path, O_RDONLY, 0);
    if (fd < 0) {
        tfail("openat read failed");
        sys_exit(1);
    }
    char buf[8];
    ssize_t n = sys_read(fd, buf, sizeof(buf));
    if (n != 3 || buf[0] != 'a' || buf[1] != 'b' || buf[2] != 'c') {
        tfail("readback mismatch");
    } else {
        tpass("openat/read/write/close");
    }
    sys_close(fd);
    sys_unlinkat(AT_FDCWD, path, 0);
    sys_exit(wave1_done());
}
