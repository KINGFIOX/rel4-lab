#include "wave1.h"

int main(void)
{
    const char msg[] = "write01: hello\n";
    ssize_t n = sys_write(1, msg, sizeof(msg) - 1);
    if (n != (ssize_t)(sizeof(msg) - 1)) {
        tfail("write stdout failed");
    } else {
        tpass("write() wrote the expected byte count");
    }
    sys_exit(wave1_done());
}
