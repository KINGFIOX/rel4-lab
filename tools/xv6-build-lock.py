#!/usr/bin/env python3
"""Compatibility entry point for the old shell lock helper.

The build lock implementation now lives in tools/tool_common.py and is imported
directly by the Python tool entry points. This file remains only so existing
paths do not disappear abruptly.
"""

from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import ROOT_DIR, BuildLock


def main() -> int:
    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    print(lock.lock_dir)
    lock.release()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
