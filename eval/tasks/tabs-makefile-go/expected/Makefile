GO ?= go
BIN := bin/server

.PHONY: build test clean

build:
	$(GO) build -trimpath -o $(BIN) ./cmd/server

test:
	$(GO) vet ./...
	$(GO) test ./...

clean:
	rm -rf bin
