.PHONY: all build build-plugin build-plugin-dev build-cli build-cli-dev install-plugin dev run check clean

TARGET_WASM = wasm32-wasip1
DIST_DIR = dist
PLUGIN_WASM = $(DIST_DIR)/verij_plugin.wasm
VERIJ_DATA_DIR ?= $(if $(XDG_DATA_HOME),$(XDG_DATA_HOME),$(HOME)/.local/share)/verij

all: dev
build: dev

$(DIST_DIR):
	mkdir -p $(DIST_DIR)

build-plugin: $(DIST_DIR)
	cargo build -p verij-plugin --target $(TARGET_WASM) --release
	cp target/$(TARGET_WASM)/release/verij-plugin.wasm $(PLUGIN_WASM)

build-plugin-dev: $(DIST_DIR)
	cargo build -p verij-plugin --target $(TARGET_WASM)
	cp target/$(TARGET_WASM)/debug/verij-plugin.wasm $(PLUGIN_WASM)

build-cli:
	cargo build -p verij-cli --release

build-cli-dev:
	cargo build -p verij-cli

# Upgrade the exporter path used by normal Zellij load_plugins configuration.
# Existing sessions still need start-or-reload-plugin for this same URL.
install-plugin: build-plugin
	mkdir -p "$(VERIJ_DATA_DIR)"
	install -m 644 "$(PLUGIN_WASM)" "$(VERIJ_DATA_DIR)/verij_plugin.wasm.tmp"
	mv "$(VERIJ_DATA_DIR)/verij_plugin.wasm.tmp" "$(VERIJ_DATA_DIR)/verij_plugin.wasm"

dev: build-plugin build-cli
	@echo "Build complete. Artifacts ready:"
	@echo "  CLI: target/release/verij"
	@echo "  WASM: $(PLUGIN_WASM)"

check:
	cargo check -p verij-cli
	cargo check -p verij-plugin --target $(TARGET_WASM)

run: dev
	target/release/verij start

clean:
	cargo clean
	rm -rf $(DIST_DIR)
