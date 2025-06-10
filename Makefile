# -- linting --------------------------------------------------------------------------------------

.PHONY: clippy
clippy: ## Runs Clippy with configs
	cargo clippy -- -D warnings

.PHONY: format-check
format-check: ## Runs Format using nightly toolchain but only in check mode
	cargo +nightly fmt --all --check

# --- building ------------------------------------------------------------------------------------
.PHONY: build
build: ## Builds with default parameters
	cargo build --release

# --- testing -------------------------------------------------------------------------------------

.PHONY: test-build
test-build: ## Build the test binary
	cargo nextest run --cargo-profile test-dev --no-run


.PHONY: test
test: ## Run all tests
	cargo nextest run --profile default --cargo-profile test-dev
