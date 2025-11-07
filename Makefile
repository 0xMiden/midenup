# -- linting --------------------------------------------------------------------------------------

.PHONY: clippy
clippy: ## Runs Clippy with configs
	cargo clippy -- -D warnings

.PHONY: fix
fix: ## Runs Fix with configs
	cargo fix --allow-staged --allow-dirty --all-targets

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
test: ## Run all tests, except integration
	cargo nextest run -- --skip integration

.PHONY: integration-test
integration-test: ## Run all integration tests
	cargo nextest run integration

# --- building ------------------------------------------------------------------------------------

.PHONY: build
build: ## Builds with default parameters
	cargo build

.PHONY: build-release
build-release: ## Builds with release profile
	cargo build --release

# --- docs ----------------------------------------------------------------------------------------

.PHONY: serve-docs
serve-docs: ## Builds docusaurus documentation & serves documentation site
	$(MAKE) -C docs/
