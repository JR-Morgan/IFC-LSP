# IFC for Zed

Zed extension for IFC (ISO 10303-21 STEP) files. Highlighting comes from the Tree-sitter
grammars in `../grammars/`; go to definition, hover and diagnostics come from the `ifc-lsp`
language server in `../server/`. EXPRESS schema files (`.exp`) are highlighted too, and the
server's go-to-definition opens the embedded schemas as cached files under the user's cache
directory (`%LOCALAPPDATA%\ifc-lsp\schemas`, `~/Library/Caches/ifc-lsp/schemas` or
`~/.cache/ifc-lsp/schemas`).

## How the server is found

1. `lsp.ifc-lsp.binary.path` in Zed's `settings.json`
2. `ifc-lsp` on the PATH
3. The latest archive from the GitHub release of the repository named in `src/lib.rs`
   (`ifc-lsp-<tag>-<target>.tar.gz` or `.zip`, produced by `.github/workflows/release.yml`)

Settings example:

```json
{
  "lsp": {
    "ifc-lsp": {
      "binary": { "path": "C:/src/ifc/server/target/release/ifc-lsp.exe" },
      "initialization_options": {
        "extraSchemaDirectories": ["C:/schemas"],
        "diagnostics": { "maxProblems": 500 }
      }
    }
  },
  // Zed skips language servers for buffers with lines longer than this (default 20000).
  "global_lsp_settings": { "max_buffer_line_length": 100000000 },
  // Folding comes from the server; Zed otherwise folds by indentation, which IFC lacks.
  "languages": {
    "IFC": { "document_folding_ranges": "on" },
    "EXPRESS": { "document_folding_ranges": "on" }
  }
}
```

## Developing

Zed compiles the extension itself (Rust → `wasm32-wasip2`) and clones the grammar
repositories, so the grammar entries in `extension.toml` must point at a Git repository and
a commit that contains the generated `src/parser.c` files.

1. Commit this repository and push it, then put the commit SHA in both `rev` fields in
   `extension.toml` (and fix `repository` if the GitHub location differs). For purely local
   work the repository can be a `file:///` URL to your checkout, e.g.
   `file:///C:/src/IFC-LSP`, still with a committed
   SHA.
2. Build the server (`cargo build --release` in `../server`) and either put it on PATH or set
   `lsp.ifc-lsp.binary.path`.
3. In Zed run `zed: install dev extension` and pick this `clients/zed/` directory. If the
   registry extension "IFC" (id `ifc`, a different project) is installed, uninstall it first:
   both claim `.ifc` files and the language name IFC.
4. Raise `global_lsp_settings.max_buffer_line_length` (default 20 000). Zed does not attach
   language servers to a buffer whose longest line exceeds it, and only says so at debug log
   level; IFC files often have lines of 50 000+ characters. See the settings example above.
5. Open an `.ifc` file. The Zed log (`zed: open log`) shows the server start-up.

After changing a grammar, run `npx tree-sitter-cli generate` in its directory, commit, and
update `rev`. Query files (`highlights.scm`, `brackets.scm`, `outline.scm`) live in
`languages/<name>/` and take effect on the next dev-extension install.

## Publishing

Zed's registry is <https://github.com/zed-industries/extensions>: every extension is a Git
submodule plus an entry in `extensions.toml`. This extension lives in a subdirectory, which
the registry supports through the `path` field. The `LICENSE` copy in this directory is
required (the registry checks the extension root, not the repository root).

First submission:

1. Tag a release of this repository first; the extension downloads `ifc-lsp` from GitHub
   releases, so without one it cannot start the server for users.
2. Fork `zed-industries/extensions` to your personal account, clone the fork, then:
   ```sh
   git submodule init && git submodule update
   git submodule add https://github.com/JR-Morgan/IFC-LSP.git extensions/ifc-lsp
   ```
3. Add to `extensions.toml` (the `version` must equal the one in `extension.toml` at the
   submodule commit):
   ```toml
   [ifc-lsp]
   submodule = "extensions/ifc-lsp"
   path = "clients/zed"
   version = "0.1.0"
   ```
4. `pnpm sort-extensions`, commit, push, open a PR against `zed-industries/extensions`.
   One extension per PR; the extension is published once the PR is merged.

Updates: bump `version` in `extension.toml` (and the `rev` fields if the grammars changed),
tag a release, then in the fork run
`git submodule update --remote extensions/ifc-lsp`, set the same version in
`extensions.toml`, and open a PR.
