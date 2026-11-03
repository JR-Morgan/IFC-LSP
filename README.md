# IFC Language Server

Language tooling for IFC files (ISO 10303-21 "STEP Part 21" text), built around a native
language server written in Rust. Works on files of any size the editor can open.

## What is in this repository

| Path | Contents |
|---|---|
| `server/` | `ifc-lsp`, the language server. Also a small CLI (`ifc-lsp lint <file>`, `ifc-lsp index-bench <file>`). |
| `clients/vscode/` | VS Code extension: TextMate grammars for IFC and EXPRESS plus a thin launcher for the server. |
| `clients/zed/` | Zed extension: language definitions, Tree-sitter queries and a WASM launcher that finds or downloads the server. |
| `grammars/` | Tree-sitter grammars for IFC and EXPRESS used by Zed. |
| `schemas/` | Unmodified buildingSMART EXPRESS schemas embedded into the server (see `schemas/NOTICE.md`). |

## Features

- **Syntax highlighting** for IFC/STEP files and EXPRESS schemas.
- **Go to definition** and find references on `#123` instance references.
- **Go to schema**: F12 on an entity name, typed value or enumeration value opens its EXPRESS declaration.
- **EXPRESS files**: any `.exp` opened in the editor gets hover, go to definition (entities, types, enum values) and folding.
- **Folding** of the `HEADER;` and `DATA;` sections, and of EXPRESS declarations.
- **Entity constructor hover**: attributes (including inherited ones) next to the actual arguments.
- **Basic schema validity checks**: unknown or abstract entities, wrong argument counts, undefined or duplicate ids, bad enumeration values, type mismatches and syntax errors.
- **Wide Schema support**: `IFC2X3`, `IFC4`, `IFC4X1`, `IFC4X2`, `IFC4X3_ADD1`, `IFC4X3_ADD2`, chosen from the file's `FILE_SCHEMA` header. Extra `.exp` files can be added through settings.
- **Fast**: a 74 MB file indexes in well under a second and edits are re-indexed incrementally.

## Debugging

Requirements: a stable Rust toolchain (<https://rustup.rs>), Node.js 24 and pnpm.

### VS Code

> Note Visual Studio Code has strict limits on the size of file that LSPs can be used for (20MB default, can be raised to 50MB) 

1. `cd clients/vscode && pnpm install`
2. Open this folder in VS Code and press F5 ("Run Extension").
   This builds the server and the client, then starts an Extension Development Host that
   uses the freshly built server from `server/target`.
3. Open an `.ifc` file. Server logs appear in the "IFC Language Server" output channel.

### Zed

1. `cd server && cargo build --release`, then put `target/release/ifc-lsp` on your PATH
   or point Zed at it:
   ```json
   { "lsp": { "ifc-lsp": { "binary": { "path": "/path/to/ifc-lsp" } } } }
   ```
2. In Zed run `zed: install dev extension` and choose the `clients/zed/` folder.
3. Raise Zed's line-length cut-off, otherwise it silently skips language servers for IFC
   buffers (the default is 20 000 characters and IFC lines are often far longer), and turn
   on server-provided folding (Zed otherwise folds by indentation, which IFC files have
   none of):
   ```json
   {
     "global_lsp_settings": { "max_buffer_line_length": 100000000 },
     "languages": {
       "IFC": { "document_folding_ranges": "on" },
       "EXPRESS": { "document_folding_ranges": "on" }
     }
   }
   ```
4. Open an `.ifc` file. Server logs are under `zed: open log`.

More detail on the Zed side, including publishing, is in `clients/zed/README.md`.

## Releasing

Releases happen only from tags. Set the same version in `server/Cargo.toml`,
`clients/vscode/package.json` and `clients/zed/extension.toml`, then:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

The release workflow checks the versions match the tag, builds the server for Linux
(x64, arm64), macOS (x64, arm64) and Windows (x64), packages one `.vsix` per platform, and
attaches everything to a GitHub release. The Zed extension downloads the server archives from
that release. If the repository variable `PUBLISH_VSCODE_MARKETPLACE` is `true`, the `.vsix`
files are also published to the Visual Studio Marketplace through Trusted Publishing.
Zed publishing is a pull request to the Zed extension registry; see `clients/zed/README.md`.

## Licence

Apache-2.0, copyright 2026 Jedd Morgan. The EXPRESS schemas in `schemas/` are
buildingSMART International's and are redistributed unmodified under their own terms; see
`schemas/NOTICE.md`.
