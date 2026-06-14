#!/usr/bin/env python3
"""Shared helpers for the repository tool entry points."""

from __future__ import annotations

import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from collections import deque
from pathlib import Path
from typing import Iterable, Sequence


ROOT_DIR = Path(__file__).resolve().parents[1]


def getenv(name: str, default: str) -> str:
    return os.environ.get(name, default)


def env_flag(name: str, default: str = "0") -> bool:
    return os.environ.get(name, default) == "1"


def qemu_smp_arg(default: str = "2") -> str:
    """Return a QEMU -smp value from the repo's SMP/NUM_NODES environment.

    `pack-image.py` uses `SMP` as a CMake boolean, while the QEMU runners need
    a CPU count. Accept both styles so a pack command such as
    `SMP=ON NUM_NODES=2` can be followed by a run command with the same
    environment.
    """
    value = os.environ.get("SMP")
    if value is None:
        return default

    stripped = value.strip()
    if stripped.isdecimal():
        return str(max(1, int(stripped)))

    normalized = stripped.upper()
    if normalized in ("ON", "TRUE", "YES", "Y"):
        return os.environ.get("NUM_NODES", default)
    if normalized in ("OFF", "FALSE", "NO", "N"):
        return "1"
    return value


def ensure_rust_log_at_least_info() -> None:
    if rust_log_level_value(os.environ.get("RUST_LOG", "")) < 3:
        os.environ["RUST_LOG"] = "info"


def rust_log_level_value(value: str) -> int:
    best: int | None = None
    for directive in value.split(","):
        level = directive.rsplit("=", 1)[-1].strip().lower()
        numeric = {
            "off": 0,
            "error": 1,
            "warn": 2,
            "warning": 2,
            "info": 3,
            "debug": 4,
            "trace": 5,
        }.get(level)
        if numeric is not None:
            best = numeric if best is None else max(best, numeric)
    return 3 if best is None else best


def bare_metal_tool_env() -> dict[str, str]:
    env = os.environ.copy()
    for key in list(env):
        if key.startswith("NIX_HARDENING_ENABLE") or key.startswith("NIX_LDFLAGS_HARDEN"):
            env[key] = ""
    env["NIX_HARDENING_ENABLE"] = ""
    env["NIX_HARDENING_ENABLE_riscv64_none_elf"] = ""
    env["NIX_LDFLAGS_HARDEN"] = ""
    env["NIX_LDFLAGS_HARDEN_riscv64_none_elf"] = ""
    return env


def log(prefix: str, message: str) -> None:
    print(f"[{prefix}] {message}", file=sys.stderr, flush=True)


def die(prefix: str, message: str, code: int = 1) -> None:
    log(prefix, f"ERROR: {message}")
    raise SystemExit(code)


def require_file(prefix: str, path: Path, message: str | None = None) -> None:
    if not path.is_file():
        die(prefix, message or f"missing file: {path}")


def require_dir(prefix: str, path: Path, message: str | None = None) -> None:
    if not path.is_dir():
        die(prefix, message or f"missing directory: {path}")


RISCV_ELF_MACHINE = 243
LOONGARCH64_ELF_MACHINE = 258
ELF_TYPE_EXECUTABLE = 2
RISCV_EFLAGS_FLOAT_ABI_MASK = 0x6
RISCV_EFLAGS_FLOAT_ABI_SOFT = 0x0
LOONGARCH64_EFLAGS_ABI_MASK = 0x7
LOONGARCH64_EFLAGS_ABI_SOFT_FLOAT = 0x1


def require_xv6_user_elf(prefix: str, target, path: Path) -> None:
    expected_machines = {
        "riscv64": RISCV_ELF_MACHINE,
        "loongarch64": LOONGARCH64_ELF_MACHINE,
    }
    expected_machine = expected_machines.get(target.name)
    if expected_machine is None:
        die(prefix, f"unsupported xv6 user ELF target: {target.name}")
    try:
        data = path.read_bytes()
    except FileNotFoundError:
        die(prefix, f"xv6 user ELF not found: {path}")
    if (
        len(data) < 64
        or data[0:4] != b"\x7fELF"
        or data[4] != 2
        or data[5] != 1
        or int.from_bytes(data[16:18], "little") != ELF_TYPE_EXECUTABLE
        or int.from_bytes(data[18:20], "little") != expected_machine
    ):
        die(prefix, f"expected a little-endian executable {target.name} xv6 user ELF: {path}")

    flags = int.from_bytes(data[48:52], "little")
    if target.name == "riscv64":
        if (flags & RISCV_EFLAGS_FLOAT_ABI_MASK) != RISCV_EFLAGS_FLOAT_ABI_SOFT:
            die(
                prefix,
                (
                    f"RISC-V xv6 user ELF must use the soft-float ABI: {path} "
                    f"has e_flags={flags:#x}"
                ),
            )
        return

    if target.name != "loongarch64":
        return
    if (flags & LOONGARCH64_EFLAGS_ABI_MASK) != LOONGARCH64_EFLAGS_ABI_SOFT_FLOAT:
        die(
            prefix,
            (
                f"LoongArch64 xv6 user ELF must use the soft-float ABI: {path} "
                f"has e_flags={flags:#x}"
            ),
        )


def command_exists(command: str) -> bool:
    return shutil.which(command) is not None


def run(
    cmd: Sequence[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    stdout=None,
    stderr=None,
) -> None:
    subprocess.run(
        list(cmd),
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        stdout=stdout,
        stderr=stderr,
        check=True,
    )


def output(cmd: Sequence[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    return subprocess.check_output(
        list(cmd),
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        text=True,
    )


def install_file(src: Path, dst: Path, mode: int = 0o644) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    os.chmod(dst, mode)


LOONGARCH_XV6_USYS_PL = """#!/usr/bin/perl -w

# Generated by repository tooling for LoongArch64 xv6 user payloads.

print "# generated by usys.pl - do not edit\\n";
print "#include \\"kernel/syscall.h\\"\\n";

sub entry {
    my $prefix = "sys_";
    my $name = shift;
    if ($name eq "sbrk") {
        print ".global $prefix$name\\n";
        print "$prefix$name:\\n";
    } else {
        print ".global $name\\n";
        print "$name:\\n";
    }
    print " li.d \\$a7, SYS_${name}\\n";
    print " syscall 0\\n";
    print " ret\\n";
}

entry("fork");
entry("exit");
entry("wait");
entry("pipe");
entry("read");
entry("write");
entry("close");
entry("kill");
entry("exec");
entry("open");
entry("mknod");
entry("unlink");
entry("fstat");
entry("link");
entry("mkdir");
entry("chdir");
entry("dup");
entry("getpid");
entry("sbrk");
entry("pause");
entry("uptime");
"""

LOONGARCH_XV6_RISCV_H = """#ifndef __ASSEMBLER__

static inline uint64
r_sp()
{
  uint64 x;
  asm volatile("or %0, $sp, $zero" : "=r"(x));
  return x;
}

#endif // __ASSEMBLER__

#define PGSIZE  4096
#define PGSHIFT 12

#define PGROUNDUP(sz)  (((sz) + PGSIZE - 1) & ~(PGSIZE - 1))
#define PGROUNDDOWN(a) (((a)) & ~(PGSIZE - 1))

#define MAXVA (1L << (9 + 9 + 9 + 12 - 1))
"""


def default_xv6_dir_for_target(target) -> Path:
    requested = ROOT_DIR / "third_party" / target.xv6_dir_name
    if requested.is_dir():
        return requested
    if target.name == "loongarch64":
        return ROOT_DIR / "third_party" / "xv6-riscv"
    return requested


def default_xv6_out_dir(target) -> Path:
    return ROOT_DIR / "target" / "xv6compat" / target.name


def prepare_xv6_dir_for_target(prefix: str, target, source_dir: Path, out_dir: Path) -> Path:
    if target.name != "loongarch64":
        return source_dir

    build_dir = Path(
        os.environ.get(
            "XV6_LOONGARCH_BUILD_DIR",
            str(out_dir / "xv6-loongarch64-user"),
        )
    )
    if source_dir.resolve() != build_dir.resolve():
        build_dir.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source_dir, build_dir, dirs_exist_ok=True)

    usys_pl = build_dir / "user" / "usys.pl"
    user_ld = build_dir / "user" / "user.ld"
    riscv_h = build_dir / "kernel" / "riscv.h"
    require_file(prefix, user_ld, f"xv6 user linker script not found: {user_ld}")
    usys_pl.write_text(LOONGARCH_XV6_USYS_PL)
    riscv_h.write_text(LOONGARCH_XV6_RISCV_H)
    linker = user_ld.read_text()
    linker = re.sub(
        r'OUTPUT_ARCH\(\s*"?riscv"?\s*\)',
        "OUTPUT_ARCH(loongarch)",
        linker,
        count=1,
    )
    user_ld.write_text(linker)
    return build_dir


def remove_files(paths: Iterable[Path]) -> None:
    for path in paths:
        try:
            path.unlink()
        except FileNotFoundError:
            pass


def touch(path: Path) -> None:
    path.touch()


def process_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


class BuildLock:
    """Directory lock compatible with the previous xv6-build-lock shell helper."""

    def __init__(self, root_dir: Path = ROOT_DIR):
        self.root_dir = root_dir
        self.lock_dir = Path(
            os.environ.get(
                "XV6_BUILD_LOCK_DIR",
                str(root_dir / "target" / "xv6compat" / ".build.lock"),
            )
        )
        self.acquired = False

    def acquire(self) -> "BuildLock":
        if os.environ.get("XV6_BUILD_LOCK_HELD") == "1":
            os.environ["XV6_BUILD_LOCK_ACQUIRED"] = "0"
            return self

        self.lock_dir.parent.mkdir(parents=True, exist_ok=True)
        while True:
            try:
                self.lock_dir.mkdir()
                break
            except FileExistsError:
                pid_path = self.lock_dir / "pid"
                holder = None
                try:
                    holder = int(pid_path.read_text().strip())
                except (FileNotFoundError, ValueError):
                    holder = None
                if holder is not None and not process_alive(holder):
                    shutil.rmtree(self.lock_dir, ignore_errors=True)
                    continue
                time.sleep(0.2)

        (self.lock_dir / "pid").write_text(f"{os.getpid()}\n")
        os.environ["XV6_BUILD_LOCK_DIR_ACTIVE"] = str(self.lock_dir)
        os.environ["XV6_BUILD_LOCK_ACQUIRED"] = "1"
        os.environ["XV6_BUILD_LOCK_HELD"] = "1"
        self.acquired = True
        return self

    def release(self) -> None:
        if not self.acquired:
            return
        shutil.rmtree(self.lock_dir, ignore_errors=True)
        os.environ["XV6_BUILD_LOCK_ACQUIRED"] = "0"
        os.environ.pop("XV6_BUILD_LOCK_HELD", None)
        os.environ.pop("XV6_BUILD_LOCK_DIR_ACTIVE", None)
        self.acquired = False

    def __enter__(self) -> "BuildLock":
        return self.acquire()

    def __exit__(self, _exc_type, _exc, _tb) -> None:
        self.release()


def xv6_user_cflags(
    xv6_dir: Path,
    march: str,
    mabi: str,
    include_dot: bool = False,
    code_model: str | None = "medany",
) -> list[str]:
    include = "." if include_dot else str(xv6_dir)
    flags = [
        "-Wall",
        "-Werror",
        "-Wno-unknown-attributes",
        "-O",
        "-fno-omit-frame-pointer",
        "-ggdb",
        "-gdwarf-2",
        f"-march={march}",
        f"-mabi={mabi}",
        "-std=gnu99",
        "-MD",
        "-ffreestanding",
        "-fno-common",
        "-nostdlib",
        "-Wno-main",
        "-fno-builtin-strncpy",
        "-fno-builtin-strncmp",
        "-fno-builtin-strlen",
        "-fno-builtin-memset",
        "-fno-builtin-memmove",
        "-fno-builtin-memcmp",
        "-fno-builtin-log",
        "-fno-builtin-bzero",
        "-fno-builtin-strchr",
        "-fno-builtin-exit",
        "-fno-builtin-malloc",
        "-fno-builtin-putc",
        "-fno-builtin-free",
        "-fno-builtin-memcpy",
        "-fno-builtin-printf",
        "-fno-builtin-fprintf",
        "-fno-builtin-vprintf",
        f"-I{include}",
        "-fno-stack-protector",
        "-fno-pie",
        "-no-pie",
    ]
    if code_model is not None:
        flags.insert(12, f"-mcmodel={code_model}")
    return flags


def c_string_literal(value: str) -> str:
    out = []
    for ch in value:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ord(ch) < 32 or ord(ch) >= 127:
            out.append(f"\\x{ord(ch):02x}")
        else:
            out.append(ch)
    return "".join(out)


def file_has_regex(path: Path, pattern: str) -> bool:
    rx = re.compile(pattern)
    try:
        with path.open("r", errors="replace") as f:
            return any(rx.search(line) for line in f)
    except FileNotFoundError:
        return False


def last_regex_line(path: Path, pattern: str) -> str:
    rx = re.compile(pattern)
    last = ""
    try:
        with path.open("r", errors="replace") as f:
            for line in f:
                if rx.search(line):
                    last = line.rstrip("\n")
    except FileNotFoundError:
        pass
    return last


def count_regex_lines(path: Path, pattern: str) -> int:
    rx = re.compile(pattern)
    try:
        with path.open("r", errors="replace") as f:
            return sum(1 for line in f if rx.search(line))
    except FileNotFoundError:
        return 0


def tail_lines(path: Path, count: int) -> list[str]:
    try:
        with path.open("r", errors="replace") as f:
            return [line.rstrip("\n") for line in deque(f, maxlen=count)]
    except FileNotFoundError:
        return []


class LoggedProcess:
    def __init__(
        self,
        cmd: Sequence[str],
        log_file: Path,
        *,
        verbose: bool = False,
        stdin_path: Path | None = None,
        stdin_delay_until: tuple[Path, str] | None = None,
    ):
        self.cmd = list(cmd)
        self.log_file = log_file
        self.verbose = verbose
        self.stdin_path = stdin_path
        self.stdin_delay_until = stdin_delay_until
        self.proc: subprocess.Popen[bytes] | None = None
        self._log_handle = None
        self._stdin_handle = None
        self._thread: threading.Thread | None = None
        self._stdin_thread: threading.Thread | None = None

    def start(self) -> subprocess.Popen[bytes]:
        self.log_file.parent.mkdir(parents=True, exist_ok=True)
        self._log_handle = self.log_file.open("wb")
        delayed_stdin = self.stdin_path is not None and self.stdin_delay_until is not None
        self._stdin_handle = self.stdin_path.open("rb") if self.stdin_path and not delayed_stdin else None
        stdin = subprocess.PIPE if delayed_stdin else self._stdin_handle
        if self.verbose:
            self.proc = subprocess.Popen(
                self.cmd,
                stdin=stdin,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            self._thread = threading.Thread(target=self._pump_output, daemon=True)
            self._thread.start()
        else:
            self.proc = subprocess.Popen(
                self.cmd,
                stdin=stdin,
                stdout=self._log_handle,
                stderr=subprocess.STDOUT,
            )
        if delayed_stdin:
            self._stdin_thread = threading.Thread(target=self._delayed_stdin, daemon=True)
            self._stdin_thread.start()
        return self.proc

    def _delayed_stdin(self) -> None:
        assert self.proc is not None
        assert self.proc.stdin is not None
        assert self.stdin_path is not None
        assert self.stdin_delay_until is not None
        trigger_path, trigger_pattern = self.stdin_delay_until
        while self.proc.poll() is None and not file_has_regex(trigger_path, trigger_pattern):
            time.sleep(0.05)
        if self.proc.poll() is not None:
            return
        try:
            with self.stdin_path.open("rb") as f:
                shutil.copyfileobj(f, self.proc.stdin)
            self.proc.stdin.close()
        except (BrokenPipeError, OSError):
            pass

    def _pump_output(self) -> None:
        assert self.proc is not None
        assert self.proc.stdout is not None
        assert self._log_handle is not None
        while True:
            chunk = self.proc.stdout.read(4096)
            if not chunk:
                break
            self._log_handle.write(chunk)
            self._log_handle.flush()
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()

    def terminate(self) -> None:
        if self.proc is None or self.proc.poll() is not None:
            return
        try:
            self.proc.terminate()
            time.sleep(0.2)
            if self.proc.poll() is None:
                self.proc.kill()
        except ProcessLookupError:
            pass

    def close(self) -> None:
        if self._thread is not None:
            self._thread.join(timeout=1.0)
        if self._stdin_thread is not None:
            self._stdin_thread.join(timeout=1.0)
        if self._stdin_handle is not None:
            self._stdin_handle.close()
            self._stdin_handle = None
        if self._log_handle is not None:
            self._log_handle.close()
            self._log_handle = None


def install_signal_cleanup(cleanup):
    def handler(signum, _frame):
        cleanup()
        raise SystemExit(128 + signum)

    old_int = signal.getsignal(signal.SIGINT)
    old_term = signal.getsignal(signal.SIGTERM)
    signal.signal(signal.SIGINT, handler)
    signal.signal(signal.SIGTERM, handler)
    return old_int, old_term


def restore_signal_cleanup(old_handlers) -> None:
    old_int, old_term = old_handlers
    signal.signal(signal.SIGINT, old_int)
    signal.signal(signal.SIGTERM, old_term)
