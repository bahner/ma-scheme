MA_FILES  := $(wildcard *.ma)
MD_FILES  := $(wildcard *.md)
CID_FILES := $(MA_FILES:.ma=.cid)

PREFIX    ?= /usr/local
BINDIR    ?= $(PREFIX)/bin

.PHONY: all build release install lint publish cids clean

all: build

# ── Rust binary ───────────────────────────────────────────────────────────────

build:
	cargo build

release:
	cargo build --release

install: release
	install -m 755 target/release/zscheme $(BINDIR)/zscheme

# ── Lint ──────────────────────────────────────────────────────────────────────

lint:
	mdl $(MD_FILES)

# ── IPFS ──────────────────────────────────────────────────────────────────────

# Publish all .ma files to IPFS and write their CIDs to matching .cid files.
publish: $(CID_FILES)

%.cid: %.ma
	@echo "Publishing $<…"
	@ipfs add -q --cid-version 1 "$<" | tee "$@"
	@echo "  → $< : $$(cat $@)"

# Print all stored CIDs.
cids:
	@for f in $(CID_FILES); do \
		name=$$(basename $$f .cid); \
		if [ -f "$$f" ]; then \
			printf '%-30s %s\n' "$$name" "$$(cat $$f)"; \
		else \
			printf '%-30s (not published)\n' "$$name"; \
		fi; \
	done

clean:
	@echo "Nothing to clean (keep .cid files in repo)"
