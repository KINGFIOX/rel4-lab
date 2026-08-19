#include "wave1.h"

int main(void)
{
    char buf[128];
    long ret = sys_getcwd(buf, sizeof(buf));
    if (ret <= 0 || buf[0] != '/') {
        tfail("getcwd failed");
        sys_exit(1);
    }
    if (sys_chdir("/tmp") != 0) {
        tfail("chdir /tmp failed");
        sys_exit(1);
    }
    char tmp[128];
    ret = sys_getcwd(tmp, sizeof(tmp));
    if (ret <= 0 || strcmp(tmp, "/tmp") != 0) {
        tfail("getcwd after chdir mismatch");
    } else {
        tpass("getcwd/chdir");
    }
    sys_chdir("/");
    sys_exit(wave1_done());
}
