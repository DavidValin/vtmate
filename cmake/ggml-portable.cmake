# Pins ggml (built by whisper-rs-sys) to a fixed x86-64-v3 instruction set.
#
# Consumed via the CMAKE_PROJECT_INCLUDE env var, which whisper-rs-sys' build
# script forwards to CMake as a -D define (it forwards every CMAKE_* variable),
# the same route as cmake/static-msvc.toolchain.cmake. CMake includes this file
# as the last step of every project() call, i.e. before ggml declares its
# GGML_* options, so the cache values set here win over ggml's defaults.
#
# Why: ggml defaults to GGML_NATIVE=ON, i.e. -march=native (CPU detection and
# /arch on MSVC). The release binaries are built on GitHub-hosted runners, so
# the instruction set depended on whichever Xeon the job landed on: an AMX
# capable runner compiled in ggml's AMX backend, whose init then prints
# "AMX is not ready to be used!" to stderr on every machine without AMX, and an
# AVX-512 runner produced a binary that dies with SIGILL on CPUs without it.
# Match the `-C target-cpu=x86-64-v3` the Rust side already uses: AVX2, FMA,
# F16C, BMI2 - Haswell (2013) and later, every Zen.
#
# Arm is left alone: it has no AMX, and GGML_NATIVE=OFF there would drop NEON
# dotprod/fp16 without a replacement baseline.

if (CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|AMD64|amd64)$")
  set(GGML_NATIVE OFF CACHE BOOL "ggml: optimize the build for the current system" FORCE)
  foreach(opt SSE42 AVX AVX2 FMA F16C BMI2)
    set(GGML_${opt} ON CACHE BOOL "ggml: enable ${opt}" FORCE)
  endforeach()
  foreach(opt AVX_VNNI AVX512 AVX512_VBMI AVX512_VNNI AVX512_BF16 AMX_TILE AMX_INT8 AMX_BF16)
    set(GGML_${opt} OFF CACHE BOOL "ggml: enable ${opt}" FORCE)
  endforeach()
endif()
