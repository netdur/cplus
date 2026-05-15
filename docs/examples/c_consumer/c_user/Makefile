# Reference Makefile for the c_consumer example.
#
# Builds:
#   make           — build the C+ library AND the C user; default target
#   make c_user    — only the C binary (assumes the library is already built)
#   make check     — build + run, verifying every export prints the expected value
#   make clean     — delete all artifacts (both library and C binary)
#
# Toolchain assumptions:
#   - cpc on PATH (or set CPC=...). The library build invokes cpc directly.
#   - clang on PATH. We don't use cc here so the example stays predictable
#     across macOS / Linux — both ship clang.

CPC   ?= cpc
CC    ?= clang

MATHLIB_DIR  := ../mathlib
MATHLIB_RELEASE := $(MATHLIB_DIR)/target/release
MATHLIB_HEADER  := $(MATHLIB_RELEASE)/mathlib.h
MATHLIB_STATIC  := $(MATHLIB_RELEASE)/libmathlib.a

# Detect dynamic-library extension. macOS uses .dylib, Linux uses .so.
# Other platforms aren't supported by Phase 5 yet (see the design note).
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
    DYLIB_EXT := dylib
    # Add an rpath so the binary finds the dylib at run time relative to
    # itself. Production builds would install the dylib to a system path.
    RPATH_FLAG := -Wl,-rpath,@executable_path/$(MATHLIB_RELEASE)
else
    DYLIB_EXT := so
    RPATH_FLAG := -Wl,-rpath,$$ORIGIN/$(MATHLIB_RELEASE)
endif

MATHLIB_DYLIB := $(MATHLIB_RELEASE)/libmathlib.$(DYLIB_EXT)

# Two build targets: statically linked or dynamically linked. Default is
# static so the binary is self-contained for the smoke test.
.PHONY: all check clean lib c_user dyn

all: c_user

# Build the C+ library. `crate-type = "both"` produces .a, .dylib/.so,
# and the auto-generated header.
lib: $(MATHLIB_STATIC) $(MATHLIB_HEADER)

$(MATHLIB_STATIC) $(MATHLIB_HEADER):
	cd $(MATHLIB_DIR) && $(CPC) build --release

# Statically linked C consumer — no rpath needed.
c_user: c_user.c lib
	$(CC) -Wall -Wextra -I$(MATHLIB_RELEASE) c_user.c $(MATHLIB_STATIC) -o c_user

# Dynamically linked variant, for users who want to verify the .dylib/.so
# also works. Not required by `make check`.
c_user_dyn: c_user.c lib
	$(CC) -Wall -Wextra -I$(MATHLIB_RELEASE) c_user.c \
	    -L$(MATHLIB_RELEASE) -lmathlib \
	    $(RPATH_FLAG) -o c_user_dyn

# `make check` is the success test: build, run, expect exit code 0
# (which means every assertion inside c_user.c passed).
check: c_user
	./c_user
	@echo "OK"

clean:
	rm -f c_user c_user_dyn
	cd $(MATHLIB_DIR) && rm -rf target
