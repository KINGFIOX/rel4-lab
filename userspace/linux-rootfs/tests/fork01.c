#include "wave1.h"

int main(void)
{
    pid_t parent = sys_getpid();
    pid_t child = sys_clone(SIGCHLD, 0);
    if (child < 0) {
        tfail("clone/fork failed");
        sys_exit(1);
    }
    if (child == 0) {
        if (sys_getppid() != parent) {
            sys_exit(2);
        }
        sys_exit(0);
    }
    int status = 0;
    pid_t waited = sys_wait4(-1, &status, 0, 0);
    if (waited != child) {
        tfail("wait4 returned unexpected pid");
    } else if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        tfail("child did not exit 0");
    } else {
        tpass("fork/clone and wait4");
    }
    sys_exit(wave1_done());
}
