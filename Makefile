# The same commands as `npm run ...`, for people who reach for make.  `make` alone lists them.
.DEFAULT_GOAL := help

##@ Help

.PHONY: help
help: # Display this help.
	@awk 'BEGIN {FS = ":.*#"; printf "Usage:\n  make \033[34m<target>\033[0m\n"} /^[a-zA-Z_0-9-]+:.*?#/ { printf "  \033[34m%-12s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) }' $(MAKEFILE_LIST)

##@ Getting started

.PHONY: setup
setup: # Install everything a fresh clone needs: Rust, the formatter and linter, npm packages, git hooks.
	npm run setup

.PHONY: run
run: # Build and open the desktop app.
	npm run app

##@ Build

.PHONY: build
build: # Build the backend and the interface.
	npm run build

.PHONY: dist
dist: # Build the Windows installer into dist/.
	npm run dist

##@ Testing and linting

.PHONY: test
test: # Run every test.
	npm test

.PHONY: fmt
fmt: # Format all code (cargo fmt, prettier).
	npm run pr -- --fix --only=format

.PHONY: lint
lint: # Lint all code (clippy with warnings denied, and the project's naming rule).
	npm run pr -- --only=lint

.PHONY: pr
pr: # Everything a pull request must pass: format check, lint, types, tests.
	npm run pr

.PHONY: clean
clean: # Remove build output.
	cargo clean
	rm -rf apps/web/.next dist
