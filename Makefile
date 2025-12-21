WASMTARGET = wasm32-unknown-unknown

proj_dir := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
dist := $(proj_dir)www
release_target := target/$(WASMTARGET)/release/peg-solitaire.wasm

.PHONY: all

all: wasm

wasm: | $(dist)
	cargo build --target $(WASMTARGET) --release
	wasm-bindgen --out-dir $(dist) --target web $(release_target)
	wasm-opt -all $(dist)/peg-solitaire_bg.wasm -Os -o $(dist)/peg-solitaire_bg_opt.wasm
	mv $(dist)/peg-solitaire_bg_opt.wasm $(dist)/peg-solitaire_bg.wasm
	cp index.html $(dist)
	cp -r assets/ www/

$(dist):
	@mkdir -p $@

install-deps:
	rustup target add $(WASMTARGET)
	cargo install -f wasm-bindgen-cli
