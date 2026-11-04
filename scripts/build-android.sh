#!/usr/bin/env bash
set -euo pipefail
: "${TARGET:?}" "${CLANG_TARGET:?}" "${ANDROID_API:?}" "${TOOLCHAIN:?}"
ndk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:?}}/ndk/27.2.12479018"
ndk_bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
export ANDROID_NDK_HOME="$ndk"
export PATH="$ndk_bin:$PATH"
# Explicit wrappers also work for the experimental RISC-V ABI, for which the
# NDK does not necessarily ship a target/API-prefixed convenience wrapper.
wrapper_dir="$PWD/target/ndk-tools/$TARGET"
mkdir -p "$wrapper_dir"
# rustc passes -nodefaultlibs, which also suppresses Clang's compiler-rt.
# NDK libatomic.a is only a placeholder; OpenSSL needs the real __atomic_*.
compiler_runtime="$("$ndk_bin/clang" --target="$CLANG_TARGET$ANDROID_API" -print-libgcc-file-name)"
test -f "$compiler_runtime"
{
  printf '#!/usr/bin/env bash\n'
  printf 'for arg in "$@"; do\n  if [[ "$arg" == -nodefaultlibs ]]; then\n'
  printf '    exec "%s/clang" "$@" --target=%s%s "%s"\n' "$ndk_bin" "$CLANG_TARGET" "$ANDROID_API" "$compiler_runtime"
  printf '  fi\ndone\n'
  printf 'exec "%s/clang" "$@" --target=%s%s\n' "$ndk_bin" "$CLANG_TARGET" "$ANDROID_API"
} > "$wrapper_dir/cc"
chmod +x "$wrapper_dir/cc"
target_key="${TARGET//-/_}"
printf -v linker_key 'CARGO_TARGET_%s_LINKER' "${target_key^^}"
export "$linker_key=$wrapper_dir/cc"
export "CC_$target_key=$wrapper_dir/cc"
export "AR_$target_key=$ndk_bin/llvm-ar"
# cc/OpenSSL may infer generic ARM from the wrapper name. Keep ARMv7 codegen;
# ARMv4 fallback emits legacy __sync_synchronize not supplied by this NDK.
if [[ "$TARGET" == armv7-linux-androideabi ]]; then
  export CFLAGS_armv7_linux_androideabi='-march=armv7-a -mfloat-abi=softfp'
fi
# Rust Android targets select static, non-PIE executables with crt-static.
# Do not override the driver with -static-pie: rustc also emits -no-pie.
export RUSTFLAGS='-C target-feature=+crt-static -C relocation-model=static -C link-arg=-Wl,-z,max-page-size=16384'
extra=()
if [[ "$TOOLCHAIN" == nightly ]]; then
  extra=(-Z build-std=std,panic_abort)
fi
# Exercise rustc + the actual NDK linker before building OpenSSL and the app.
# Tier-3 targets use the same build-std options as the final build.
probe_dir="$wrapper_dir/probe"
mkdir -p "$probe_dir/src"
cat > "$probe_dir/Cargo.toml" <<'TOML'
[package]
name = "ysunethelper-android-link-probe"
version = "0.0.0"
edition = "2024"
[workspace]
TOML
printf 'fn main() { println!("{}", std::env::consts::ARCH); }\n' > "$probe_dir/src/main.rs"
cargo +"$TOOLCHAIN" build --release --target "$TARGET" --manifest-path "$probe_dir/Cargo.toml" \
  --target-dir "$PWD/target" "${extra[@]}"
probe="$PWD/target/$TARGET/release/ysunethelper-android-link-probe"
"$ndk_bin/llvm-readelf" -lW "$probe" > "$probe_dir/program-headers.txt"
"$ndk_bin/llvm-readelf" -dW "$probe" > "$probe_dir/dynamic-section.txt"
if grep -q INTERP "$probe_dir/program-headers.txt" || grep -q '(NEEDED)' "$probe_dir/dynamic-section.txt"; then
  echo 'Android link probe unexpectedly depends on a dynamic loader/shared library' >&2
  exit 1
fi
if [[ "$TARGET" == riscv64-linux-android ]]; then
  # openssl-src does not yet map this Rust target. Build the very same locked
  # OpenSSL sources explicitly instead of disabling HTTPS for RISC-V Android.
  cargo fetch --locked
  source_dir="$(cargo metadata --locked --format-version 1 | python3 -c 'import json,sys,pathlib; p=next(p for p in json.load(sys.stdin)["packages"] if p["name"]=="openssl-src"); print(pathlib.Path(p["manifest_path"]).parent / "openssl")')"
  openssl_build="$PWD/target/openssl-android-riscv64"
  openssl_install="$PWD/target/openssl-install-android-riscv64"
  mkdir -p "$openssl_build"
  (
    cd "$openssl_build"
    CC="$wrapper_dir/cc" AR="$ndk_bin/llvm-ar" RANLIB="$ndk_bin/llvm-ranlib" \
      perl "$source_dir/Configure" linux64-riscv64 no-shared no-tests no-module no-dso no-asm --prefix="$openssl_install" --libdir=lib
    make -j2
    make install_sw
  )
  export OPENSSL_DIR="$openssl_install" OPENSSL_STATIC=1 OPENSSL_NO_VENDOR=1
fi
cargo +"$TOOLCHAIN" build --locked --release --target "$TARGET" "${extra[@]}"
