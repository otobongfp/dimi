# Dev workflow for Dimi. Run `make` or `make help` to list targets.
#
# Layout reminder: `runtime/` is the dimi-runtime crate (kernel, services,
# pipelines) and `crates/isfi/` is the filesystem-index library — both
# standalone. `apps/workspace/` is the desktop app: `src-tauri/` (the `dimi`
# Tauri crate) and `src/` (the React frontend) together. See dimi-docs/ for
# the architecture these commands assume.

SHELL := /bin/bash
CARGO := cargo
PNPM  := pnpm -C apps/workspace
DB    := $(HOME)/.dimi/dimi.db

# A dev shell may export CC/AR/CFLAGS/CXXFLAGS for an unrelated project
# (e.g. pointed at a wasm32 toolchain) — that breaks every native C/C++
# build script in this workspace (llama-cpp-sys-2, aws-lc-sys, tesseract-sys,
# rusqlite's bundled sqlite, ...). `.cargo/config.toml` force-clears
# CFLAGS/CXXFLAGS (safe on every OS: empty extra flags are always a no-op),
# but CC/AR can't be handled the same way there — there's no single value
# correct on Linux, macOS, *and* Windows (MSVC uses cl.exe/lib.exe, not
# cc/ar), and Cargo's [env] table has no per-OS conditioning. `unexport`
# solves this properly: it removes whatever the calling shell set, for
# every recipe below, on every OS `make` runs on — no guessed replacement
# value, so nothing here can be "wrong" for a given platform.
unexport CC AR CFLAGS CXXFLAGS

.PHONY: help setup doctor dev build build-release bundle \
        migrate db-reset db-shell \
        test test-lib test-integration test-frontend test-all \
        fmt lint check clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

## --- Setup ---

setup: ## Install frontend deps and check native toolchain prerequisites
	$(PNPM) install
	./scripts/check-prerequisites.sh

doctor: ## Check that cmake/clang/pnpm/tesseract are installed (no build)
	./scripts/check-prerequisites.sh

## --- Run ---

dev: ## Run the app in development mode (Tauri + Vite, hot reload)
	$(PNPM) tauri dev

## --- Database ---

migrate: ## Apply pending SQLite migrations against the dev database (fast — no model loading)
	$(CARGO) run -p dimi-runtime --bin migrate

db-reset: ## Delete the local dev database (recreated with fresh migrations on next boot)
	rm -f "$(DB)"
	@echo "Removed $(DB) — it will be recreated on next boot."

db-shell: ## Open a sqlite3 shell on the dev database
	sqlite3 "$(DB)"

## --- Build ---

build: ## Build the runtime crate (debug)
	$(CARGO) build -p dimi-runtime

build-release: ## Build the runtime crate (release)
	$(CARGO) build -p dimi-runtime --release

bundle: ## Package a distributable app (.dmg/.msi/.AppImage/.deb per platform)
	$(PNPM) tauri build

## --- Tests ---

test: test-lib ## Alias for test-lib

test-lib: ## Run backend unit/integration tests (fast — no real model or network needed)
	$(CARGO) test -p dimi-runtime --lib

test-integration: ## Run the full golden-path E2E test (needs DIMI_DEV_MODEL_PATH, loads a real model — slow)
	@if [ -z "$$DIMI_DEV_MODEL_PATH" ]; then \
		echo "Set DIMI_DEV_MODEL_PATH=/path/to/model.gguf to run this target" >&2; exit 1; \
	fi
	$(CARGO) test -p dimi-runtime --test golden_path -- --ignored --nocapture

test-frontend: ## Type-check the frontend (tsc --noEmit)
	$(PNPM) exec tsc --noEmit

test-all: test-lib test-frontend ## Run all fast tests (backend + frontend); skips the slow model-dependent ones

## --- Quality ---

fmt: ## Format Rust code
	$(CARGO) fmt --all

lint: ## Lint Rust (clippy) and type-check the frontend
	$(CARGO) clippy -p dimi-runtime -p dimi
	$(PNPM) exec tsc --noEmit

check: ## Fast compile check of both Rust crates (no codegen)
	$(CARGO) check -p dimi-runtime -p dimi

## --- Cleanup ---

clean: ## Remove Rust and frontend build artifacts
	$(CARGO) clean
	rm -rf apps/workspace/dist

.DEFAULT_GOAL := help
