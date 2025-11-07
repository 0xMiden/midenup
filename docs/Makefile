# --- internal docs -------------------------------------------------------------------------------
all: serve-docs

.PHONY: build-docs
build-docs:
	npm ci
	npm run build:dev

.PHONY: serve-docs
serve-docs: build-docs ## Builds docusaurus documentation & serves documentation site
	npm run start:dev
