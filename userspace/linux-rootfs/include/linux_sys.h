#ifndef LINUX_SYS_H
#define LINUX_SYS_H

typedef long ssize_t;
typedef unsigned long size_t;
typedef int pid_t;

#define NULL ((void *)0)

#define SYS_GETCWD 17
#define SYS_DUP 23
#define SYS_DUP3 24
#define SYS_MKDIRAT 34
#define SYS_UNLINKAT 35
#define SYS_CHDIR 49
#define SYS_OPENAT 56
#define SYS_CLOSE 57
#define SYS_PIPE2 59
#define SYS_LSEEK 62
#define SYS_READ 63
#define SYS_WRITE 64
#define SYS_EXIT 93
#define SYS_EXIT_GROUP 94
#define SYS_WAITID 95
#define SYS_NANOSLEEP 101
#define SYS_CLOCK_GETTIME 113
#define SYS_UNAME 160
#define SYS_GETPID 172
#define SYS_GETPPID 173
#define SYS_GETUID 174
#define SYS_GETGID 176
#define SYS_BRK 214
#define SYS_CLONE 220
#define SYS_EXECVE 221
#define SYS_WAIT4 260

#define AT_FDCWD (-100)
#define AT_REMOVEDIR 0x200

#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0100
#define O_EXCL 0200
#define O_TRUNC 01000

#define SIGCHLD 17
#define WNOHANG 1

#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1

#define UTS_LEN 65

struct utsname {
    char sysname[UTS_LEN];
    char nodename[UTS_LEN];
    char release[UTS_LEN];
    char version[UTS_LEN];
    char machine[UTS_LEN];
    char domainname[UTS_LEN];
};

struct timespec {
    long tv_sec;
    long tv_nsec;
};

long syscall6(long n, long a0, long a1, long a2, long a3, long a4, long a5);
long syscall3(long n, long a0, long a1, long a2);
long syscall2(long n, long a0, long a1);
long syscall1(long n, long a0);
long syscall0(long n);

ssize_t sys_write(int fd, const void *buf, size_t count);
ssize_t sys_read(int fd, void *buf, size_t count);
int sys_openat(int dirfd, const char *path, int flags, int mode);
int sys_close(int fd);
int sys_mkdirat(int dirfd, const char *path, int mode);
int sys_unlinkat(int dirfd, const char *path, int flags);
int sys_chdir(const char *path);
long sys_getcwd(char *buf, size_t size);
int sys_dup(int fd);
int sys_dup3(int oldfd, int newfd, int flags);
int sys_pipe2(int fds[2], int flags);
int sys_uname(struct utsname *buf);
int sys_clock_gettime(int clock_id, struct timespec *ts);
pid_t sys_getpid(void);
pid_t sys_getppid(void);
int sys_getuid(void);
int sys_getgid(void);
pid_t sys_clone(unsigned long flags, void *stack);
int sys_execve(const char *path, char *const argv[], char *const envp[]);
pid_t sys_wait4(pid_t pid, int *status, int options, void *rusage);
void sys_exit(int status);
void sys_exit_group(int status);

int uname(struct utsname *buf);
void *memset(void *s, int c, size_t n);
int strcmp(const char *a, const char *b);
size_t strlen(const char *s);
void prints(const char *s);
void printn(const char *s, size_t n);
int WIFEXITED(int status);
int WEXITSTATUS(int status);

#endif
