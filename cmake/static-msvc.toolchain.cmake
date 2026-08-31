# Forces the static MSVC runtime (/MT) on every CMake sub-build.
#
# Consumed via the CMAKE_TOOLCHAIN_FILE env var, which the `cmake` crate
# forwards to CMake. This is how espeak-rs-sys and whisper-rs-sys — which
# build their own C/C++ deps and ignore our flags - get /MT instead of /MD.
# Without it espeak-ng links MSVCRT and drags vcruntime140.dll into the exe.

set(CMAKE_POLICY_DEFAULT_CMP0091 NEW)
set(CMAKE_MSVC_RUNTIME_LIBRARY
    "MultiThreaded$<$<CONFIG:Debug>:Debug>"
    CACHE STRING "MSVC runtime library" FORCE)

# Belt and braces: CMAKE_MSVC_RUNTIME_LIBRARY only applies when the project
# honours CMP0091. Some vendored CMakeLists set policies to OLD, so append
# the flag directly too.
foreach(lang C CXX)
  foreach(cfg "" _RELEASE _RELWITHDEBINFO _MINSIZEREL)
    string(APPEND CMAKE_${lang}_FLAGS${cfg} " /MT /wd4875")
  endforeach()
  string(APPEND CMAKE_${lang}_FLAGS_DEBUG " /MTd")
endforeach()
