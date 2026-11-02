//! Zed extension: locates or downloads `ifc-lsp` and starts it for IFC and
//! EXPRESS buffers. Highlighting comes from the Tree-sitter grammars declared in
//! `extension.toml`.

use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

/// GitHub repository whose releases carry the server archives built by
/// `.github/workflows/release.yml`.
const RELEASE_REPO: &str = "JR-Morgan/IFC-LSP";
const SERVER_ID: &str = "ifc-lsp";

struct IfcExtension {
    cached_binary: Option<String>,
}

impl IfcExtension {
    fn binary_path(&mut self, id: &LanguageServerId, worktree: &zed::Worktree) -> Result<String> {
        // 1. `lsp.ifc-lsp.binary.path` in Zed settings.
        if let Some(path) = LspSettings::for_worktree(SERVER_ID, worktree)
            .ok()
            .and_then(|s| s.binary)
            .and_then(|b| b.path)
        {
            return Ok(path);
        }
        // 2. A server already on PATH (e.g. a `cargo install` or a development build).
        if let Some(path) = worktree.which(SERVER_ID) {
            return Ok(path);
        }
        // 3. A copy this extension downloaded earlier.
        if let Some(path) = &self.cached_binary
            && std::fs::metadata(path).is_ok_and(|m| m.is_file())
        {
            return Ok(path.clone());
        }
        // 4. Download the latest release archive for this platform.
        zed::set_language_server_installation_status(
            id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = zed::latest_github_release(
            RELEASE_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;
        let (os, arch) = zed::current_platform();
        let target = match (os, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "aarch64-apple-darwin",
            (zed::Os::Mac, _) => "x86_64-apple-darwin",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "aarch64-unknown-linux-gnu",
            (zed::Os::Linux, _) => "x86_64-unknown-linux-gnu",
            (zed::Os::Windows, _) => "x86_64-pc-windows-msvc",
        };
        let (extension, file_type) = match os {
            zed::Os::Windows => ("zip", zed::DownloadedFileType::Zip),
            _ => ("tar.gz", zed::DownloadedFileType::GzipTar),
        };
        let asset_name = format!("ifc-lsp-{}-{target}.{extension}", release.version);
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| format!("release {} has no asset {asset_name}", release.version))?;

        let version_dir = format!("ifc-lsp-{}", release.version);
        let binary = format!(
            "{version_dir}/ifc-lsp{}",
            if os == zed::Os::Windows { ".exe" } else { "" }
        );
        if !std::fs::metadata(&binary).is_ok_and(|m| m.is_file()) {
            zed::set_language_server_installation_status(
                id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(&asset.download_url, &version_dir, file_type)
                .map_err(|e| format!("failed to download {asset_name}: {e}"))?;
            zed::make_file_executable(&binary)?;
            // Drop older versions so the extension directory does not grow forever.
            if let Ok(entries) = std::fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with("ifc-lsp-")
                        && name != version_dir.as_str()
                    {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
        zed::set_language_server_installation_status(
            id,
            &zed::LanguageServerInstallationStatus::None,
        );
        self.cached_binary = Some(binary.clone());
        Ok(binary)
    }
}

impl zed::Extension for IfcExtension {
    fn new() -> Self {
        Self {
            cached_binary: None,
        }
    }

    fn language_server_command(
        &mut self,
        id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree(SERVER_ID, worktree).ok();
        let args = settings
            .as_ref()
            .and_then(|s| s.binary.as_ref())
            .and_then(|b| b.arguments.clone())
            .unwrap_or_else(|| vec!["--stdio".to_string()]);
        Ok(zed::Command {
            command: self.binary_path(id, worktree)?,
            args,
            env: worktree.shell_env(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        // `lsp.ifc-lsp.initialization_options` in Zed settings is passed through, e.g.
        // { "extraSchemaDirectories": ["/path/to/exp"], "diagnostics": { "maxProblems": 500 } }
        Ok(LspSettings::for_worktree(SERVER_ID, worktree)
            .ok()
            .and_then(|s| s.initialization_options))
    }
}

zed::register_extension!(IfcExtension);
