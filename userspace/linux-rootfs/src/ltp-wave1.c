#include "linux_sys.h"

static const char *const tests[] = {
    "/uname01",
    "/exit01",
    "/write01",
    "/open01",
    "/getpid01",
    "/fork01",
    "/mkdir01",
    "/chdir01",
    "/getuid01",
    "/clock_gettime01",
    "/dup01",
    "/pipe01",
    0,
};

static int run_one(const char *path)
{
    pid_t child = sys_clone(SIGCHLD, 0);
    if (child < 0) {
        prints("TFAIL clone ");
        prints(path);
        prints("\n");
        return 1;
    }
    if (child == 0) {
        char *argv[2];
        argv[0] = (char *)path;
        argv[1] = 0;
        sys_execve(path, argv, 0);
        prints("TFAIL exec ");
        prints(path);
        prints("\n");
        sys_exit(127);
    }
    int status = 0;
    pid_t waited = sys_wait4(child, &status, 0, 0);
    if (waited != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        prints("TFAIL ");
        prints(path);
        prints("\n");
        return 1;
    }
    return 0;
}

int main(void)
{
    int failed = 0;
    unsigned i;
    for (i = 0; tests[i] != 0; i++) {
        prints("ltp-wave1: run ");
        prints(tests[i]);
        prints("\n");
        failed += run_one(tests[i]);
    }
    if (failed == 0) {
        prints("ltp-wave1: ok\n");
        sys_exit(0);
    }
    prints("ltp-wave1: fail\n");
    sys_exit(1);
}
