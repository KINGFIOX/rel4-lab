#include "linux_sys.h"

long syscall6(long n, long a0, long a1, long a2, long a3, long a4, long a5)
{
    register long t0 __asm__("a7") = n;
    register long r0 __asm__("a0") = a0;
    register long r1 __asm__("a1") = a1;
    register long r2 __asm__("a2") = a2;
    register long r3 __asm__("a3") = a3;
    register long r4 __asm__("a4") = a4;
    register long r5 __asm__("a5") = a5;
    __asm__ volatile("ecall"
                     : "+r"(r0)
                     : "r"(t0), "r"(r1), "r"(r2), "r"(r3), "r"(r4), "r"(r5)
                     : "memory");
    return r0;
}

long syscall3(long n, long a0, long a1, long a2)
{
    return syscall6(n, a0, a1, a2, 0, 0, 0);
}

long syscall2(long n, long a0, long a1)
{
    return syscall6(n, a0, a1, 0, 0, 0, 0);
}

long syscall1(long n, long a0)
{
    return syscall6(n, a0, 0, 0, 0, 0, 0);
}

long syscall0(long n)
{
    return syscall6(n, 0, 0, 0, 0, 0, 0);
}

ssize_t sys_write(int fd, const void *buf, size_t count)
{
    return syscall3(SYS_WRITE, fd, (long)buf, (long)count);
}

ssize_t sys_read(int fd, void *buf, size_t count)
{
    return syscall3(SYS_READ, fd, (long)buf, (long)count);
}

int sys_openat(int dirfd, const char *path, int flags, int mode)
{
    return (int)syscall6(SYS_OPENAT, dirfd, (long)path, flags, mode, 0, 0);
}

int sys_close(int fd)
{
    return (int)syscall1(SYS_CLOSE, fd);
}

int sys_mkdirat(int dirfd, const char *path, int mode)
{
    return (int)syscall3(SYS_MKDIRAT, dirfd, (long)path, mode);
}

int sys_unlinkat(int dirfd, const char *path, int flags)
{
    return (int)syscall3(SYS_UNLINKAT, dirfd, (long)path, flags);
}

int sys_chdir(const char *path)
{
    return (int)syscall1(SYS_CHDIR, (long)path);
}

long sys_getcwd(char *buf, size_t size)
{
    return syscall2(SYS_GETCWD, (long)buf, (long)size);
}

int sys_dup(int fd)
{
    return (int)syscall1(SYS_DUP, fd);
}

int sys_dup3(int oldfd, int newfd, int flags)
{
    return (int)syscall3(SYS_DUP3, oldfd, newfd, flags);
}

int sys_pipe2(int fds[2], int flags)
{
    return (int)syscall2(SYS_PIPE2, (long)fds, flags);
}

int sys_uname(struct utsname *buf)
{
    return (int)syscall1(SYS_UNAME, (long)buf);
}

int sys_clock_gettime(int clock_id, struct timespec *ts)
{
    return (int)syscall2(SYS_CLOCK_GETTIME, clock_id, (long)ts);
}

pid_t sys_getpid(void)
{
    return (pid_t)syscall0(SYS_GETPID);
}

pid_t sys_getppid(void)
{
    return (pid_t)syscall0(SYS_GETPPID);
}

int sys_getuid(void)
{
    return (int)syscall0(SYS_GETUID);
}

int sys_getgid(void)
{
    return (int)syscall0(SYS_GETGID);
}

pid_t sys_clone(unsigned long flags, void *stack)
{
    return (pid_t)syscall6(SYS_CLONE, (long)flags, (long)stack, 0, 0, 0, 0);
}

int sys_execve(const char *path, char *const argv[], char *const envp[])
{
    return (int)syscall3(SYS_EXECVE, (long)path, (long)argv, (long)envp);
}

pid_t sys_wait4(pid_t pid, int *status, int options, void *rusage)
{
    return (pid_t)syscall6(SYS_WAIT4, pid, (long)status, options, (long)rusage, 0, 0);
}

void sys_exit(int status)
{
    syscall1(SYS_EXIT, status);
    for (;;) {
    }
}

void sys_exit_group(int status)
{
    syscall1(SYS_EXIT_GROUP, status);
    for (;;) {
    }
}

int uname(struct utsname *buf)
{
    return sys_uname(buf);
}

void *memset(void *s, int c, size_t n)
{
    unsigned char *p = s;
    size_t i;
    for (i = 0; i < n; i++) {
        p[i] = (unsigned char)c;
    }
    return s;
}

int strcmp(const char *a, const char *b)
{
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}

size_t strlen(const char *s)
{
    size_t n = 0;
    while (s[n] != 0) {
        n++;
    }
    return n;
}

void printn(const char *s, size_t n)
{
    sys_write(1, s, n);
}

void prints(const char *s)
{
    printn(s, strlen(s));
}

int WIFEXITED(int status)
{
    return (status & 0x7f) == 0;
}

int WEXITSTATUS(int status)
{
    return (status >> 8) & 0xff;
}
