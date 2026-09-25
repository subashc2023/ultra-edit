GO ?= go
BIN := bin/server

.PHONY: build test clean

build:
	$(GO) build -o $(BIN) ./cmd/server

test:
	$(GO) test ./...

clean:
	rm -rf bin
