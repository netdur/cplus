# rt — the Rust port. cargo does the real work; the release profile lives in
# Cargo.toml. There is no contraction knob: rustc never fuses a*b+c, so this
# corresponds to the other ports' contract=off setting.
BIN := target/release/rt
.PHONY: all run clean
all:
	cargo build --release
run: all
	./$(BIN)
clean:
	cargo clean
	rm -f out.ppm
