# -- linting --------------------------------------------------------------------------------------

.PHONY: clippy
clippy: ## Runs Clippy with configs
	cargo clippy --all --all-targets -- -D clippy::all -D warnings

.PHONY: fix
fix: ## Runs Fix with configs
	cargo fix --allow-staged --allow-dirty --all-targets

.PHONY: format
format: format-manifest ## Runs Format using nightly toolchain
	cargo +nightly fmt --all

.PHONY: format-check
format-check: ## Runs Format using nightly toolchain but only in check mode
	cargo +nightly fmt --all --check

.PHONY: check-manifest
check-manifest: update-manifest
	bin/update-manifest --manifest-path manifest/channel-manifest.json check

.PHONY: format-manifest
format-manifest: update-manifest
	bin/update-manifest --manifest-path manifest/channel-manifest.json format

.PHONY: lint
lint: format clippy ## Runs all linting tasks at once (Clippy, formatting)

# --- testing -------------------------------------------------------------------------------------

.PHONY: test-build
test-build: ## Build the test binary
	cargo nextest run --workspace --no-run

.PHONY: test
test: ## Run all tests, except integration
	cargo nextest run --workspace --no-fail-fast -- --skip integration

.PHONY: integration-test
integration-test: ## Run integration tests, excluding the slow pre-release checks
	cargo nextest run --workspace --no-fail-fast -E 'test(/integration_/) and not test(/prerelease/)'

.PHONY: recovery-test
recovery-test: ## Run restart-recovery tests, which need the fault-injection abort points compiled in
	cargo nextest run --workspace --features fault-injection --no-fail-fast -E 'test(/integration_recovery_/)'

.PHONY: prerelease-test
prerelease-test: ## Run pre-release checks against the real manifest (slow: builds real components)
	cargo nextest run --workspace --no-fail-fast -E 'test(/prerelease/)'

# --- building ------------------------------------------------------------------------------------

.PHONY: check
check: ## Perform a check build with default parameters
	cargo check

.PHONY: build
build: ## Builds with default parameters
	cargo build

.PHONY: build-release
build-release: ## Builds with release profile
	cargo build --release

.PHONY: install
install: ## Installs midenup in release configuration
	cargo install --locked --path . --force --bin midenup

.PHONY: update-manifest
update-manifest: ## Builds the update-manifest tool
	cargo +nightly -Z unstable-options build -p update-manifest --artifact-dir bin

# --- docs ----------------------------------------------------------------------------------------
.PHONY: serve-docs
serve-docs: ## Builds docusaurus documentation & serves documentation site
	$(MAKE) -C docs/
