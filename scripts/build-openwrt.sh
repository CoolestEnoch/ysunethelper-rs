#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"
sdk_archive='openwrt-sdk-24.10.4-ramips-mt7621_gcc-13.3.0_musl.Linux-x86_64.tar.zst'
sdk_sha='e857be9826272de900094246d2cea51c409ebc3b8da428471a4333814112a498'
sdk_parent="$PWD/target/openwrt-sdk"
mkdir -p "$sdk_parent"
# Reuse a local archive only after verifying the pinned release checksum.
if ! printf '%s  %s\n' "$sdk_sha" "$sdk_parent/$sdk_archive" | sha256sum --check --status 2>/dev/null; then
  curl --fail --location --retry 3 --retry-all-errors \
    "https://downloads.openwrt.org/releases/24.10.4/targets/ramips/mt7621/$sdk_archive" -o "$sdk_parent/$sdk_archive"
fi
printf '%s  %s\n' "$sdk_sha" "$sdk_parent/$sdk_archive" | sha256sum --check
sdk="$sdk_parent/${sdk_archive%.tar.zst}"
if [[ ! -d "$sdk" ]]; then tar --zstd -xf "$sdk_parent/$sdk_archive" -C "$sdk_parent"; fi
(
  cd "$sdk"
  # Pin base feed, including OpenSSL, to the same release as the SDK.
  printf 'src-git base https://github.com/openwrt/openwrt.git;v24.10.4\n' > feeds.conf
  ./scripts/feeds update base
  ./scripts/feeds install -p base openssl
  printf 'CONFIG_ALL_NONSHARED=n\nCONFIG_ALL_KMODS=n\nCONFIG_ALL=n\nCONFIG_PACKAGE_libopenssl=y\n' > .config
  make defconfig
  make package/openssl/compile -j2 V=s
)
shopt -s nullglob
toolchains=("$sdk"/staging_dir/toolchain-mipsel_24kc_gcc-*_musl)
targets=("$sdk"/staging_dir/target-mipsel_24kc_musl)
[[ ${#toolchains[@]} == 1 && ${#targets[@]} == 1 ]]
toolchain="${toolchains[0]}"
export STAGING_DIR="${targets[0]}"
export PATH="$toolchain/bin:$PATH"
export CC_mipsel_unknown_linux_musl=mipsel-openwrt-linux-musl-gcc
export AR_mipsel_unknown_linux_musl=mipsel-openwrt-linux-musl-ar
export CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER=mipsel-openwrt-linux-musl-gcc
export OPENSSL_NO_VENDOR=1 OPENSSL_STATIC=0
export OPENSSL_DIR="$STAGING_DIR/usr"
export OPENSSL_LIB_DIR="$STAGING_DIR/usr/lib"
export OPENSSL_INCLUDE_DIR="$STAGING_DIR/usr/include"
export CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_PANIC=abort
export CARGO_PROFILE_RELEASE_STRIP=symbols
# The built-in Rust target already uses soft-float; keep that ABI and use
# the SDK's shared libc, OpenSSL, and libgcc_s for the MIPS32r2 / 24Kc CPU.
export RUSTFLAGS="-C target-cpu=mips32r2 -C target-feature=-crt-static -C link-self-contained=no -C link-arg=-lgcc -C link-arg=-Wl,--dynamic-linker=/lib/ld-musl-mipsel-sf.so.1 -L native=$STAGING_DIR/usr/lib -C link-arg=-Wl,-rpath-link,$STAGING_DIR/usr/lib"
cd "$project_dir"
cargo +nightly build --locked --release --target mipsel-unknown-linux-musl \
  -Z build-std=std,panic_abort -Z build-std-features=optimize_for_size

# These outputs are consumed by the QEMU smoke check in the release workflow.
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  printf 'sysroot=%s\nlibrary_path=%s\n' "$toolchain" "$STAGING_DIR/usr/lib" >> "$GITHUB_OUTPUT"
fi
