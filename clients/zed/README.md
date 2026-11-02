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
3. In Zed run `zed: install dev extension` and pick this `clients/zed/` directory.
4. Open an `.ifc` file. The Zed log (`zed: open log`) shows the server start-up.

After changing a grammar, run `npx tree-sitter-cli generate` in its directory, commit, and
update `rev`. Query files (`highlights.scm`, `brackets.scm`, `outline.scm`) live in
`languages/<name>/` and take effect on the next dev-extension install.

## Publishing

Zed extensions are published by adding the extension as a Git submodule to
<https://github.com/zed-industries/extensions> and listing it in `extensions.toml`. This
directory must then be the root of its own repository (or the submodule must point at a
repository whose root contains `extension.toml`); a subtree split of `clients/zed/` is the usual
way to do that from a monorepo.
