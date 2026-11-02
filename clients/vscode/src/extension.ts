import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

// VS Code never synchronises text models above this size with extensions
// (TextModel._MODEL_SYNC_LIMIT), so the language server cannot see them.
const SYNC_LIMIT = 50 * 1024 * 1024;
// Above this size (or 300k lines) VS Code disables tokenisation unless
// editor.largeFileOptimizations is false.
const TOKENIZE_LIMIT = 20 * 1024 * 1024;

// Embedded EXPRESS schemas are served by the server as read-only documents.
const SCHEMA_SCHEME = "ifc-schema";
const SCHEMA_SOURCE_REQUEST = "ifc/schemaSource";

let client: LanguageClient | undefined;
const warnedFiles = new Set<string>();

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel("IFC Language Server");
  context.subscriptions.push(output);

  const start = async () => {
    const serverPath = resolveServerPath(context);
    if (!serverPath) {
      const pick = await vscode.window.showErrorMessage(
        "ifc-lsp executable not found. Set `ifc.serverPath` or build the server (see README).",
        "Open Settings",
      );
      if (pick) {
        void vscode.commands.executeCommand("workbench.action.openSettings", "ifc.serverPath");
      }
      return;
    }
    output.appendLine(`Starting ${serverPath}`);
    const serverOptions: ServerOptions = {
      command: serverPath,
      transport: TransportKind.stdio,
      options: { env: { ...process.env, IFC_LSP_LOG: process.env.IFC_LSP_LOG ?? "info" } },
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [{ language: "ifc" }, { scheme: SCHEMA_SCHEME, language: "express" }],
      synchronize: { configurationSection: "ifc" },
      initializationOptions: serverSettings(),
      outputChannel: output,
    };
    client = new LanguageClient("ifc", "IFC Language Server", serverOptions, clientOptions);
    await client.start();
  };

  const schemaProvider: vscode.TextDocumentContentProvider = {
    async provideTextDocumentContent(uri) {
      if (!client) {
        return "-- IFC language server is not running";
      }
      const result = await client.sendRequest<{ text: string | null }>(SCHEMA_SOURCE_REQUEST, {
        uri: uri.toString(),
      });
      return result?.text ?? `-- unknown schema ${uri.path}`;
    },
  };

  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(SCHEMA_SCHEME, schemaProvider),
    vscode.commands.registerCommand("ifc.restartServer", async () => {
      await client?.stop();
      client = undefined;
      await start();
    }),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("ifc.serverPath")) {
        void vscode.commands.executeCommand("ifc.restartServer");
      }
    }),
    vscode.window.tabGroups.onDidChangeTabs((e) => {
      for (const tab of [...e.opened, ...e.changed]) {
        void warnIfLarge(tab);
      }
    }),
  );
  for (const group of vscode.window.tabGroups.all) {
    for (const tab of group.tabs) {
      void warnIfLarge(tab);
    }
  }

  await start();
}

export async function deactivate(): Promise<void> {
  await client?.stop();
  client = undefined;
}

function serverSettings(): unknown {
  const cfg = vscode.workspace.getConfiguration("ifc");
  return {
    extraSchemaDirectories: cfg.get<string[]>("extraSchemaDirectories", []),
    diagnostics: { maxProblems: cfg.get<number>("diagnostics.maxProblems", 1000) },
    // Embedded schemas are shown as `ifc-schema:` documents provided by this client;
    // other editors get them as cached files instead.
    schemaDocuments: "virtual",
  };
}

function resolveServerPath(context: vscode.ExtensionContext): string | undefined {
  const exe = process.platform === "win32" ? "ifc-lsp.exe" : "ifc-lsp";
  const configured = vscode.workspace.getConfiguration("ifc").get<string>("serverPath", "").trim();
  if (configured) {
    return configured;
  }
  const candidates = [
    path.join(context.extensionPath, "server", `${process.platform}-${process.arch}`, exe),
    process.env.IFC_LSP_SERVER ?? "",
    path.join(context.extensionPath, "..", "..", "server", "target", "release", exe),
    path.join(context.extensionPath, "..", "..", "server", "target", "debug", exe),
  ].filter((p) => p !== "");
  // In a development checkout prefer whichever build is newer.
  const existing = candidates.filter((p) => fs.existsSync(p));
  if (existing.length === 0) {
    return undefined;
  }
  if (existing.length === 1 || !existing[0].includes(`${path.sep}target${path.sep}`)) {
    return existing[0];
  }
  return existing.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs)[0];
}

async function warnIfLarge(tab: vscode.Tab): Promise<void> {
  const input = tab.input;
  if (!(input instanceof vscode.TabInputText)) {
    return;
  }
  const uri = input.uri;
  if (uri.scheme !== "file" || !/\.(ifc|stp|step|p21)$/i.test(uri.fsPath)) {
    return;
  }
  const key = uri.toString();
  if (warnedFiles.has(key)) {
    return;
  }
  let size: number;
  try {
    size = (await vscode.workspace.fs.stat(uri)).size;
  } catch {
    return;
  }
  if (size > SYNC_LIMIT) {
    warnedFiles.add(key);
    const mb = (size / 1024 / 1024).toFixed(0);
    void vscode.window.showWarningMessage(
      `${path.basename(uri.fsPath)} is ${mb} MB. VS Code does not share files over 50 MB with extensions, so IFC hover, navigation and diagnostics are unavailable for it.`,
    );
  } else if (
    size > TOKENIZE_LIMIT &&
    vscode.workspace.getConfiguration("editor").get<boolean>("largeFileOptimizations", true)
  ) {
    warnedFiles.add(key);
    const pick = await vscode.window.showInformationMessage(
      `${path.basename(uri.fsPath)} is over 20 MB, so VS Code has disabled syntax colouring. The IFC language features still work.`,
      "Enable colouring for large files",
    );
    if (pick) {
      await vscode.workspace
        .getConfiguration("editor")
        .update("largeFileOptimizations", false, vscode.ConfigurationTarget.Global);
    }
  }
}
