# rt — the C reference for examples/ray_tracer.
#
#   make            optimized build   -> build/rt
#   make run        build and render  -> ./out.ppm
#   make clean
#
# FPC selects the float-contraction policy, and it must match whatever the
# other ports use or the images will not compare. `on` is the default in all
# three toolchains; `off` costs speed and buys reproducibility across
# compilers. See the top-level README.
CC   ?= cc
FPC  ?= on
BIN  := build/rt

UNAME_M := $(shell uname -m)
ifeq ($(filter arm64 aarch64,$(UNAME_M)),)
  ARCHFLAGS := -march=native
else
  ARCHFLAGS := -mcpu=native
endif

CFLAGS := -std=c11 -O3 $(ARCHFLAGS) -fno-math-errno -ffp-contract=$(FPC) \
          -Wall -Wextra
LDLIBS := -lm

.PHONY: all run clean
all: $(BIN)

build:
	@mkdir -p build

$(BIN): src/main.c Makefile | build
	$(CC) $(CFLAGS) -o $@ src/main.c $(LDLIBS)

run: $(BIN)
	./$(BIN)

clean:
	rm -rf build out.ppm
