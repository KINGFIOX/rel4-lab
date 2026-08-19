#include "wave1.h"

int main(void)
{
    const char dir[] = "/tmp/mkdir01.d";
    if (sys_mkdirat(AT_FDCWD, dir, 0755) != 0) {
        tfail("mkdirat failed");
        sys_exit(1);
    }
    if (sys_unlinkat(AT_FDCWD, dir, AT_REMOVEDIR) != 0) {
        tfail("unlinkat rmdir failed");
    } else {
        tpass("mkdirat/unlinkat");
    }
    sys_exit(wave1_done());
}
