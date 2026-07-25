#!/usr/bin/env bash
# Checks the native toolchains building Dimi's runtime crate needs:
# a Rust toolchain (rust-toolchain.toml pins "stable"), cmake + a C/C++
# compiler (llama-cpp-sys-2 compiles llama.cpp from source), and pnpm for
# the frontend. Tesseract is optional — OcrEngine just reports Degraded
# without it (see runtime/src/services/ocr.rs); everything else still works.

set -uo pipefail

all_ok=1

check() {
  local name="$1" cmd="$2" hint="$3"
  if command -v "$cmd" >/dev/null 2>&1; then
    echo "[ok]      $name ($(command -v "$cmd"))"
  else
    echo "[MISSING] $name — $hint"
    all_ok=0
  fi
}

echo "Checking Dimi build prerequisites..."
echo

check "Rust toolchain" rustc "install via https://rustup.rs"
check "cmake"          cmake "brew install cmake   (needed to build llama.cpp)"
check "clang"          clang "install Xcode Command Line Tools: xcode-select --install"
check "pnpm"           pnpm  "brew install pnpm  or  npm install -g pnpm"

echo
if command -v tesseract >/dev/null 2>&1; then
  echo "[ok]       tesseract ($(command -v tesseract))"
else
  echo "[optional] tesseract not found — brew install tesseract"
  echo "           OCR for scanned documents/images will be Degraded, not required to build or run."
fi

echo
if [ "$all_ok" -eq 1 ]; then
  echo "All required prerequisites found."
  exit 0
else
  echo "Missing required prerequisites listed above — install them before building."
  exit 1
fi
