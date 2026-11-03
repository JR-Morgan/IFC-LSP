# IFC LSP

Language support for IFC files (ISO 10303-21 "STEP Part 21" text), backed by a native
language server written in Rust. Fast enough for real models: a 74 MB file indexes in well
under a second and edits are re-indexed incrementally.

## Features

- Syntax highlighting for IFC/STEP files and EXPRESS schemas.
- Go to definition and find references on `#123` instance references.
- Go to schema: F12 on an entity name, typed value or enumeration value opens its EXPRESS
  declaration.
- EXPRESS files: any `.exp` opened in the editor gets hover, go to definition (entities,
  types, enum values) and folding.
- Folding of the `HEADER;` and `DATA;` sections, and of EXPRESS declarations.
- Hover on an entity constructor shows its attributes, including inherited ones, next to the
  actual arguments.
- Diagnostics for obviously invalid content: unknown or abstract entities, wrong argument
  counts, undefined or duplicate ids, bad enumeration values, type mismatches and syntax
  errors. Full conformance checking (WHERE rules) is out of scope.
- Schemas embedded: IFC2X3, IFC4, IFC4X1, IFC4X2, IFC4X3_ADD1 and IFC4X3_ADD2, chosen from
  the file's `FILE_SCHEMA` header. Extra `.exp` files can be added through settings.

## Large files

VS Code itself limits what extensions can do with large documents:

| File size | What works                                                                                                                   |
| --------- | ---------------------------------------------------------------------------------------------------------------------------- |
| ≤ 20 MB   | everything                                                                                                                   |
| 20–50 MB  | everything except syntax colouring, which VS Code disables for large files unless `editor.largeFileOptimizations` is `false` |
| > 50 MB   | nothing: VS Code does not share documents above 50 MB with any extension, so the language server never sees them             |

## Settings

| Setting                       | Default | Meaning                                                      |
| ----------------------------- | ------- | ------------------------------------------------------------ |
| `ifc.serverPath`              | `""`    | Path to an `ifc-lsp` binary; empty uses the one bundled here |
| `ifc.extraSchemaDirectories`  | `[]`    | Directories with additional `.exp` schemas                   |
| `ifc.diagnostics.maxProblems` | `1000`  | Cap on reported problems per file                            |
| `ifc.trace.server`            | `off`   | LSP message tracing                                          |

## Licence

Apache-2.0. The embedded EXPRESS schemas are buildingSMART International's and are
redistributed unmodified under their own terms. Source: <https://github.com/JR-Morgan/IFC-LSP>.
