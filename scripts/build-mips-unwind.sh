#!/usr/bin/env bash
set -euo pipefail
source_dir="${1:?LLVM source directory required}"
build_dir="${2:?Build directory required}"
# CMake needs a runnable HOST interpreter, never a target-sysroot Python.
python_host="$(command -v python3)" || { echo 'Host python3 is required to configure LLVM runtimes' >&2; exit 1; }
"$python_host" -c 'import sys; assert sys.version_info >= (3, 8), "LLVM 19 requires Python 3.8+"'

# cross images have used both mipsel-unknown-linux-musl-* and
# mipsel-linux-muslsf-* tools. The Rust triple is not the GCC executable name.
cc="${CC_mipsel_unknown_linux_musl:-${CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER:-}}"
if [[ -z "$cc" && -n "${CROSS_TOOLCHAIN_PREFIX:-}" ]]; then
  cc="${CROSS_TOOLCHAIN_PREFIX}gcc"
fi
if [[ -z "$cc" ]]; then
  for candidate in mipsel-linux-muslsf-gcc mipsel-unknown-linux-musl-gcc; do
    if command -v "$candidate" >/dev/null; then cc="$candidate"; break; fi
  done
fi
cc="$(command -v "${cc:?No cross C compiler found}")"
cxx="$(command -v "${CXX_mipsel_unknown_linux_musl:-${cc%gcc}g++}")"
ar="$(command -v "${AR_mipsel_unknown_linux_musl:-${cc%gcc}ar}")"
ranlib="$(command -v "${cc%gcc}ranlib")"
machine="$("$cc" -dumpmachine)"
case "$machine" in mipsel*) ;; *) echo "Refusing non-MIPS compiler: $cc ($machine)" >&2; exit 1 ;; esac
sysroot="${CROSS_SYSROOT:-$("$cc" -print-sysroot)}"
[[ -n "$sysroot" && "$sysroot" != / && -d "$sysroot/lib" ]]
printf 'Building MIPS unwind: cc=%s cxx=%s sysroot=%s\n' "$cc" "$cxx" "$sysroot"
cmake -S "$source_dir/runtimes" -B "$build_dir" \
  -DLLVM_ENABLE_RUNTIMES=libunwind \
  -DPython3_EXECUTABLE="$python_host" \
  -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER \
  -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_DOCS=OFF \
  -DLLVM_ENABLE_PER_TARGET_RUNTIME_DIR=OFF \
  -DLLVM_DEFAULT_TARGET_TRIPLE="$machine" \
  -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR=mipsel \
  -DCMAKE_C_COMPILER="$cc" -DCMAKE_CXX_COMPILER="$cxx" \
  -DCMAKE_ASM_COMPILER="$cc" -DCMAKE_AR="$ar" -DCMAKE_RANLIB="$ranlib" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_FLAGS='-march=mips32r2 -msoft-float' \
  -DCMAKE_CXX_FLAGS='-march=mips32r2 -msoft-float' \
  -DCMAKE_ASM_FLAGS='-march=mips32r2 -msoft-float' \
  -DLIBUNWIND_ENABLE_SHARED=OFF -DLIBUNWIND_ENABLE_STATIC=ON \
  -DLIBUNWIND_INCLUDE_TESTS=OFF -DLIBUNWIND_INCLUDE_DOCS=OFF \
  -DLIBUNWIND_USE_COMPILER_RT=OFF
cmake --build "$build_dir" --target unwind_static -j2
archive="$build_dir/lib/libunwind.a"
test -s "$archive"
cp "$archive" "$sysroot/lib/libunwind.a"
# Fail during image creation if the archive cannot satisfy actual Rust unwind
# references, instead of discovering that after compiling every dependency.
cat > "$build_dir/check.c" <<'C'
extern void *_Unwind_GetIP(void *);
extern int __ucmpdi2(unsigned long long, unsigned long long);
volatile unsigned long long left = 0xffffffffffffffffULL;
volatile unsigned long long right = 1;
int main(int argc, char **argv) {
    return argc == 12345 ? (int)(long)_Unwind_GetIP(argv) : __ucmpdi2(left, right);
}
C
# Match rustc's -nodefaultlibs instead of letting the driver silently supply
# libgcc. OpenSSL calls its 64-bit comparison helpers on this 32-bit target.
"$cc" -static -march=mips32r2 -msoft-float -nodefaultlibs \
  "$build_dir/check.c" -lunwind -lc -lgcc -o "$build_dir/check"
"${cc%gcc}readelf" -A "$build_dir/check" | grep -F 'Soft float'
