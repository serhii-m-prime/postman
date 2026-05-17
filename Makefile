.PHONY: build release install fetch process publish stats clean clean-db reset db

# Якщо під час виклику make передано DEBUG=1, підставляємо --debug, інакше залишаємо пусто
DEBUG_FLAG = $(if $(DEBUG),--debug,)

# Default target
all: build

# 1. Compilation
build:
	cargo build

release:
	cargo build --release

# 2. Infrastructure
install:
	cargo run -- install

# 3. Workers (Runtime commands)
# Usage: make fetch | make fetch FEED=techcrunch | make fetch DEBUG=1
fetch:
	@if [ "$(FEED)" ]; then \
		cargo run -- fetch $(DEBUG_FLAG) --data-feed=$(FEED); \
	else \
		cargo run -- fetch $(DEBUG_FLAG); \
	fi

# Usage: make process | make process DEBUG=1
process:
	cargo run -- process $(DEBUG_FLAG)

# Usage: make publish | make publish CAT=drones | make publish DEBUG=1
publish:
	@if [ "$(CAT)" ]; then \
		cargo run -- publish $(DEBUG_FLAG) --category=$(CAT); \
	else \
		cargo run -- publish $(DEBUG_FLAG) --category=general; \
	fi

stats:
	cargo run -- stats $(DEBUG_FLAG)

# 4. Utilities
reset: clean-db install

# Шукаємо будь-який .db файл у папці data і відкриваємо його
db:
	@DB_PATH=$$(ls data/*.db 2>/dev/null | head -n 1); \
	if [ -z "$$DB_PATH" ]; then \
		echo "Database file not found in data/ directory."; \
	else \
		echo "Opening $$DB_PATH..."; \
		sqlite3 $$DB_PATH; \
	fi

clean:
	cargo clean

# Радикальне очищення всієї згенерованої дати (БД, кеші, логи, якщо вони там)
clean-db:
	rm -rf data/*
	@echo "Data directory cleaned."