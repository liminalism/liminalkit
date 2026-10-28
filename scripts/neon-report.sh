#!/usr/bin/env bash
# Which hot loops compile to NEON on aarch64 (the Raspberry Pi), read from
# the assembly. Needs only `rustup target add aarch64-unknown-linux-gnu` — no
# cross linker, no ARM machine: the link step is expected to fail after the
# assembly has been written.
#
# Counts instructions per function and how many touch NEON vector registers.
# A small function whose loop LLVM vectorized shows a large share; in a large
# function a vectorized loop can be a handful of instructions among many
# (Rotation::copy's straight copy is `ldp q / bic v.4s / stp q`), so read the
# body when the count is small. This proves the shape of the code, not its
# speed — measure speed on the device.
set -u
cd "$(dirname "$0")/.."
target=aarch64-unknown-linux-gnu
out=target/$target/release

count() { # file symbol-line-regex label
  local body total vec
  body=$(awk -v re="$2" '$0 ~ re {p=1} p {print} p && /\.cfi_endproc/ {exit}' "$1")
  total=$(grep -cE '^\s+[a-z]' <<<"$body")
  vec=$(grep -cE '\bv[0-9]+\.(16b|8b|8h|4h|4s|2s|2d)|\bq[0-9]+\b' <<<"$body")
  printf '%-40s %5s instructions  %5s NEON\n' "$3" "$total" "$vec"
}

cargo rustc -q --release -p liminalkit-raster --example neon_probe --target $target \
  -- --emit asm -C codegen-units=1 2>/dev/null
cargo rustc -q --release -p liminalkit-kms --lib --target $target \
  -- --emit asm -C codegen-units=1 2>/dev/null

probe=$(ls -t $out/examples/neon_probe-*.s 2>/dev/null | head -1)
kms=$(ls -t $out/deps/liminalkit_kms-*.s 2>/dev/null | head -1)
[ -n "$probe" ] && [ -n "$kms" ] || { echo "no assembly: is the $target target installed?"; exit 1; }

# The wrappers carry their names inside Rust's mangled symbols.
for f in probe_blend_spec probe_blend_span probe_over_span probe_max_span probe_fill; do
  count "$probe" "^_R.*${#f}${f}:\$" "$f"
done
# The copy to the glass, by mangled name: the dispatcher and its parts.
for f in copy copy_words copy_turned copy_bytes; do
  if grep -qE "^_R.*Rotation.*${#f}${f}:\$" "$kms"; then
    count "$kms" "^_R.*Rotation.*${#f}${f}:\$" "Rotation::$f"
  else
    printf '%-40s inlined into its caller\n' "Rotation::$f"
  fi
done
