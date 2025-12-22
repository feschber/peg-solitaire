TARGET_NAME := peg-solitaire
WASMTARGET := wasm32-unknown-unknown
BUILDTYPE ?= release
TARGET := target/$(WASMTARGET)/$(BUILDTYPE)/$(TARGET_NAME).wasm

PROJ_DIR := $(dir $(lastword $(MAKEFILE_LIST)))
DIST := $(PROJ_DIR)www

.PHONY: all
all: wasm

# build wasm binary
$(TARGET):
ifeq ($(BUILDTYPE),release)
	cargo build --target $(WASMTARGET) --release
else
	cargo build --target $(WASMTARGET)
endif

# generate javascript glue-code
BINDGEN_FILES = $(addprefix $(DIST)/$(TARGET_NAME),.d.ts .js _bg.wasm _bg.wasm.d.ts)
$(BINDGEN_FILES): $(TARGET) | $(DIST)
	rm -rf $(DIST)
	wasm-bindgen --out-dir $(DIST) --target web $(TARGET)

$(DIST):
	@mkdir -p $@

# optimize wasm binary
WASMOPT = $(DIST)/$(TARGET_NAME)_bg_opt.wasm
%_opt.wasm: %.wasm
	wasm-opt -all $< -Os -o $*_opt.wasm

# compress using brotli
WASMBR = $(DIST)/$(TARGET_NAME)_bg_opt.wasm.br
%.wasm.br: %.wasm
	brotli -9 -o $@ $<

# copy files to destination
.PHONY: wasm
wasm: $(BINDGEN_FILES) $(WASMBR)
	mv $(DIST)/peg-solitaire_bg_opt.wasm $(DIST)/peg-solitaire_bg.wasm || true
	mv $(DIST)/peg-solitaire_bg_opt.wasm.br $(DIST)/peg-solitaire_bg.wasm.br || true
	cp index.html $(DIST)
	cp -r assets/ $(DIST)/assets/

.PHONY: clean
clean:
	rm -rf $(DIST)

install-deps:
	rustup target add $(WASMTARGET)
	cargo install wasm-bindgen-cli || true
	cargo install wasm-server-runner || true
