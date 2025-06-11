# -- linting --------------------------------------------------------------------------------------

.PHONY: clippy
clippy: ## Runs Clippy with configs
	cargo clippy -- -D warnings

.PHONY: fix
fix: ## Runs Fix with configs
	cargo +nightly fix --allow-staged --allow-dirty --all-targets

.PHONY: format
format: ## Runs Format using nightly toolchain
	cargo +nightly fmt --all

.PHONY: format-check
format-check: ## Runs Format using nightly toolchain but only in check mode
	cargo +nightly fmt --all --check

.PHONY: lint
lint: format clippy ## Runs all linting tasks at once (Clippy, formatting)

# --- testing -------------------------------------------------------------------------------------

.PHONY: test-build
test-build: ## Build the test binary
	cargo nextest run --no-run


.PHONY: test
test: ## Run all tests
	cargo nextest run --profile default

# --- building ------------------------------------------------------------------------------------
.PHONY: build
build: ## Builds with default parameters
	cargo build
