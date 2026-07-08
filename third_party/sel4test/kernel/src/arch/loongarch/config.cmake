#
# Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
#
# SPDX-License-Identifier: GPL-2.0-only
#

config_string(
  KernelPTLevels PT_LEVELS "Number of page \
    table levels for LoongArch depends on the mode. For example there are: \
    2, 3 and 4 levels on Sv32, Sv39, Sv48 LoongArch paging modes respectively."
  DEFAULT 3
  UNDEF_DISABLED UNQUOTE
  DEPENDS "KernelArchLoongArch")

set(_KernelLoongArchExtD ON)
set(_KernelLoongArchExtF ON)
if(LLVM_TOOLCHAIN AND KernelSel4ArchLoongArch32)
  # Versions of clang we support can't compile for D double width floating
  # point. But we've found that having F but not D still leads to errors with
  # code that assumes if any floating point is enabled, both F and D are enabled.
  set(_KernelLoongArchExtD OFF)
  set(_KernelLoongArchExtF OFF)
endif()

config_option(KernelLoongArchExtF LOONGARCH_EXT_F "LoongArch extension for single-precision floating-point"
              DEFAULT ${_KernelLoongArchExtF} DEPENDS "KernelArchLoongArch")

config_option(KernelLoongArchExtD LOONGARCH_EXT_D "LoongArch extension for double-precision floating-point"
              DEFAULT ${_KernelLoongArchExtD} DEPENDS "KernelArchLoongArch")

config_option(
  KernelLoongArchUseClintMtime
  LOONGARCH_USE_CLINT_MTIME
  "When reading the timestamp \
    from the hardware, directly access the CLINT timer register (mtime) instead \
    of using the rdtime instruction. This is a performance optimization, but \
    only for platforms where executing the rdtime instruction results in a \
    trap into M-mode software which then accesses the CLINT timer register. \
    Note that this option requires S-mode access to the CLINT."
  DEFAULT OFF
  DEPENDS "KernelArchLoongArch")

# Until LoongArch has instructions to count leading/trailing zeros, we provide
# library implementations. Platforms that implement the bit manipulation
# extension can override these settings to remove the library functions from
# the image.
# In the verified configurations, we additionally define KernelClzNoBuiltin and
# KernelCtzNoBuiltin to expose the library implementations to verification.
# However, since the NoBuiltin options force the use of the library functions
# even when the platform has sutiable inline assembly, we do not make these the
# default.
if(KernelWordSize EQUAL 32)
  set(KernelClz32 ON CACHE BOOL "")
  set(KernelCtz32 ON CACHE BOOL "")
  if(KernelIsMCS)
    # Used for long division in timer calculations.
    set(KernelClz64 ON CACHE BOOL "")
  endif()
elseif(KernelWordSize EQUAL 64)
  set(KernelClz64 ON CACHE BOOL "")
  set(KernelCtz64 ON CACHE BOOL "")
endif()

if(KernelSel4ArchLoongArch32)
  set(KernelPTLevels 2 CACHE STRING "" FORCE)
endif()
if(KernelPTLevels EQUAL 2)
  if(KernelSel4ArchLoongArch32)
    # seL4 on LOONGARCH32 uses 32-bit ints for addresses,
    # so limit the maximum paddr to 32-bits.
    math(EXPR KernelPaddrUserTop "(1 << 32) - 1")
  else()
    math(EXPR KernelPaddrUserTop "1 << 34")
  endif()
elseif(KernelPTLevels EQUAL 3)
  # LoongArch technically supports 56-bit paddrs,
  # but structures.bf limits us to using 39 of those bits.
  math(EXPR KernelPaddrUserTop "1 << 39")
elseif(KernelPTLevels EQUAL 4)
  math(EXPR KernelPaddrUserTop "1 << 56")
endif()

if(KernelLoongArchExtD)
  # The D extension depends on the base single-precision
  # instruction subset F.
  set(KernelLoongArchExtF ON)
endif()

set(KernelHaveFPU ON)

# This is not supported on LoongArch
set(KernelHardwareDebugAPIUnsupported ON CACHE INTERNAL "")

add_sources(
  DEP "KernelArchLoongArch"
  PREFIX src/arch/loongarch
  CFILES c_traps.c
         idle.c
         api/faults.c
         api/benchmark.c
         kernel/boot.c
         kernel/thread.c
         kernel/vspace.c
         machine/capdl.c
         machine/hardware.c
         machine/registerset.c
         machine/io.c
         machine/fpu.c
         model/statedata.c
         object/interrupt.c
         object/objecttype.c
         object/tcb.c
         smp/ipi.c
  ASMFILES head.S traps.S idle.S)

add_bf_source_old("KernelArchLoongArch" "structures.bf" "include/arch/loongarch" "arch/object")
