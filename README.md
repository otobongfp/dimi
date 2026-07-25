# Dimi

A private, local-first AI runtime — offline document Q&A, file operations, and drafting, running entirely on-device. See [`dimi-docs/`](../dimi-docs) for the full architecture.

Layout: `runtime/` is the `dimi-runtime` crate (kernel, services, pipelines) and `crates/isfi/` is the filesystem-index library, both standalone. `apps/workspace/` is the desktop app — `src-tauri/` (the `dimi` Tauri crate) and `src/` (the React frontend).

## Prerequisites

- [Rust](https://rustup.rs) (version pinned in `rust-toolchain.toml`)
- `cmake` and a C/C++ compiler (`llama-cpp-sys-2` builds llama.cpp from source)
- [pnpm](https://pnpm.io)
- `tesseract` — optional, only needed for OCR on scanned documents/images

Check what's installed:

```sh
make doctor
```

## Setup

```sh
make setup   # installs frontend deps, checks native toolchain prerequisites
```

## Development

```sh
make dev            # run the app (Tauri + Vite, hot reload)
make test-all        # backend unit tests + frontend type-check
make lint            # clippy + type-check
```

## Building a release

```sh
make build-release   # release build of the runtime crate
make bundle           # package a distributable app (.dmg/.msi/.AppImage/.deb per platform)
```

Run `make help` for the full list of targets (database tools, individual test suites, etc).
