LOCAL_TAURI_SIGNING_KEY := $(CURDIR)/.tauri/cursor-byok.local.key

.PHONY: check dev-web dev-server dev-desktop dev-collector build-web build-server build-collector package-collector build-desktop build-docker

COLLECTOR_TARGET ?= x86_64-unknown-linux-gnu
COLLECTOR_PLATFORM ?= linux/amd64
COLLECTOR_IMAGE ?= cursor-overview-collector:local
COLLECTOR_OUT ?= dist/collector

check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace --all-targets
	npm --prefix apps/desktop run check

dev-web:
	npm --prefix apps/desktop run dev:web

dev-server:
	CURSOR_CONSOLE_DIR=apps/desktop/dist cargo run --package cursor-server --bin cursor-server

dev-desktop:
	npm --prefix apps/desktop run tauri:dev

dev-collector:
	CURSOR_OVERVIEW_COLLECTOR_ADDR=127.0.0.1:4399 \
	CURSOR_DATABASE_URL=sqlite://./overview-collector.db \
	cargo run --package cursor-server --bin cursor-overview-collector

build-web:
	npm --prefix apps/desktop run build

build-server:
	cargo build --release --package cursor-server --bin cursor-server

build-collector:
	cargo build --release --package cursor-server --bin cursor-overview-collector

# Cross-builds the collector for a Linux server and extracts the plain binary,
# e.g. `make package-collector COLLECTOR_PLATFORM=linux/amd64`.
#
# COLLECTOR_BUILDER decides the minimum glibc of the produced binary:
#   rust:1-bullseye -> glibc 2.31 (Ubuntu 20.04+, Aliyun Linux 3, Debian 11)
#   rust:1-bookworm -> glibc 2.34 (Ubuntu 22.04+, Debian 12)
COLLECTOR_BUILDER ?= rust:1-bullseye

package-collector:
	@mkdir -p "$(COLLECTOR_OUT)"
	docker build --platform "$(COLLECTOR_PLATFORM)" \
		--build-arg BUILDER_IMAGE="$(COLLECTOR_BUILDER)" \
		--build-arg RUNTIME_IMAGE="debian:bookworm-slim" \
		--target builder \
		-f server/Dockerfile.collector -t "$(COLLECTOR_IMAGE)" .
	@cid=$$(docker create --platform "$(COLLECTOR_PLATFORM)" "$(COLLECTOR_IMAGE)"); \
	docker cp "$$cid:/tmp/cursor-overview-collector" "$(COLLECTOR_OUT)/cursor-overview-collector"; \
	docker rm "$$cid" >/dev/null
	chmod 755 "$(COLLECTOR_OUT)/cursor-overview-collector"
	cp server/deploy/install-collector.sh "$(COLLECTOR_OUT)/install-collector.sh"
	chmod 755 "$(COLLECTOR_OUT)/install-collector.sh"
	@cd "$(COLLECTOR_OUT)" && shasum -a 256 cursor-overview-collector install-collector.sh > SHA256SUMS 2>/dev/null || \
	 cd "$(COLLECTOR_OUT)" && sha256sum cursor-overview-collector install-collector.sh > SHA256SUMS
	@echo "packaged into $(COLLECTOR_OUT)"

ifeq ($(OS),Windows_NT)
$(LOCAL_TAURI_SIGNING_KEY):
	@powershell -NoProfile -Command "New-Item -ItemType Directory -Force -Path '$(dir $@)' | Out-Null; & '$(CURDIR)/apps/desktop/node_modules/.bin/tauri.cmd' signer generate --ci --write-keys '$@'"

build-desktop: $(LOCAL_TAURI_SIGNING_KEY)
	@node -e "const { spawnSync } = require('node:child_process'); const result = spawnSync(process.execPath, ['node_modules/@tauri-apps/cli/tauri.js', 'build', '--bundles', 'nsis'], { cwd: 'apps/desktop', stdio: 'inherit', env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: process.argv[1], TAURI_SIGNING_PRIVATE_KEY_PASSWORD: '' } }); process.exit(result.status ?? 1)" "$(LOCAL_TAURI_SIGNING_KEY)"
else
$(LOCAL_TAURI_SIGNING_KEY):
	@install -d -m 700 "$(dir $@)"
	@apps/desktop/node_modules/.bin/tauri signer generate --ci --write-keys "$@" >/dev/null
	@chmod 600 "$@" "$@.pub"

build-desktop: $(LOCAL_TAURI_SIGNING_KEY)
	TAURI_SIGNING_PRIVATE_KEY="$(LOCAL_TAURI_SIGNING_KEY)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" npm --prefix apps/desktop run tauri:build
endif

build-docker:
	docker build --tag cursor-byok:local .
