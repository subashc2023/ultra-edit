Both files below are indented with tab characters, and Make requires recipe lines to start with a tab. Keep every existing tab, and indent new lines with tabs exactly as described.

1. `Makefile`
   - In the `build` recipe, change `$(GO) build -o $(BIN) ./cmd/server` to `$(GO) build -trimpath -o $(BIN) ./cmd/server`.
   - In the `test` target, add the recipe line `$(GO) vet ./...` directly above `$(GO) test ./...`, indented with one tab.
2. `cmd/server/main.go`
   - Change the default port `"8080"` to `"9090"`.
   - Directly below that `port = "9090"` line, add `fmt.Fprintln(os.Stderr, "PORT not set; using 9090")`, indented with two tabs like the line above it.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
