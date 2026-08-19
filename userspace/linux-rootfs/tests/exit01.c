#include "wave1.h"

int main(void)
{
    int status = 0;
    pid_t child = sys_clone(SIGCHLD, 0);
    if (child < 0) {
        tfail("clone failed");
        sys_exit(1);
    }
    if (child == 0) {
        sys_exit(7);
    }
    pid_t waited = sys_wait4(child, &status, 0, 0);
    if (waited != child) {
        tfail("wait pid mismatch");
    } else if (!WIFEXITED(status) || WEXITSTATUS(status) != 7) {
        tfail("unexpected exit status");
    } else {
        tpass("exit() returned the correct wait status");
    }
    sys_exit(wave1_done());
}
