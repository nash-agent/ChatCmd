# ChatCMD Astra Workspace custom build

This fork keeps the Astra Workspace custom runtime on the normal `main` branch.

## New Windows PC: clone -> automatic install

After cloning the repository on Windows, run the root launcher:

```text
SETUP_PC2.cmd
```

Do not manually install Node, Rust, Visual Studio Build Tools, copy the executable, copy the browser bridge, create shortcuts, or troubleshoot the local port first. The setup script performs those steps.

### What SETUP_PC2.cmd automates

It:

- checks for a compatible Node.js and installs Node.js LTS with `winget` when missing;
- checks for rustup/Rust and installs or updates stable Rust to 1.85+ when needed;
- checks for Visual Studio 2022 C++ Build Tools and installs the C++ workload when missing;
- deliberately invokes `npm.cmd`, avoiding the common Windows PowerShell policy failure on `npm.ps1`;
- runs `npm ci` and builds the web UI;
- builds the Windows x64 Rust release with the embedded web UI;
- installs `ChatCMD.exe` under `%LOCALAPPDATA%\Programs\ChatCMD-Custom`;
- copies the complete `chatgpt-extension` directory to the stable installed path;
- verifies `manifest.json` exists in that installed extension directory;
- verifies the installed executable by SHA-256;
- creates Windows Startup and Start Menu shortcuts;
- starts ChatCMD;
- requires `http://127.0.0.1:8080/api/ping` to become healthy;
- writes `HUMAN_STEPS_ONLY.txt`;
- copies the exact extension folder path to the clipboard;
- opens the exact installed extension folder and the local ChatCMD UI.

The setup is intended to be rerunnable. If Windows installs a prerequisite but the current terminal does not see its new PATH yet, reopen Terminal and run `SETUP_PC2.cmd` again.

## The browser bridge

There is one Chromium extension used by this build:

**ChatCMD ChatGPT Bridge**

It is already part of this repository in `chatgpt-extension/`. It is not an OpenAI extension and it is not downloaded separately from the Chrome Web Store.

The installer copies it to:

```text
%LOCALAPPDATA%\Programs\ChatCMD-Custom\chatgpt-extension
```

This is an unpacked Manifest V3 extension. Chromium does not provide a reliable normal-user CLI that permanently installs an arbitrary unpacked extension into an existing browser profile, so one browser action remains manual.

## Human-only steps after automatic setup succeeds

Only these steps are intentionally left for the person using the PC:

1. In Chrome, Edge, or Brave, open the extensions manager, enable **Developer mode**, click **Load unpacked**, and select the exact installed extension folder opened/copied by the installer. Confirm the extension is **ChatCMD ChatGPT Bridge**.
2. Sign in to `chatgpt.com` in that same browser profile and reload ChatGPT once.
3. In the local ChatCMD UI at `http://127.0.0.1:8080`, create a new MCP access profile/access code for this PC.
4. Register the local project folders this PC is allowed to use: **Projects -> + -> Project folder -> Choose folder**.
5. Complete any provider/account authentication required by the same external connection method used on the working PC1 setup.

Everything else that the local installer can safely automate should already be complete.

## MCP address

A newly created local access profile normally has this shape:

```text
http://127.0.0.1:8080/mcp/<token>
```

That is a normal local MCP endpoint.

Do **not** install or configure Cloudflare merely because the endpoint contains `127.0.0.1`. Match the working PC1 connection method. A public address, tunnel, reverse proxy, or other route is only needed when the actual client path requires one.

Treat the complete tokenized MCP URL as a password. Never commit it or publish it.

## Project paths

Project/workspace paths are configured per PC in the ChatCMD UI. Do not hard-code PC1 drive letters or absolute paths in source.

Use:

**Projects -> + -> Project folder -> Choose folder**

The selected folder becomes the task-scoped project workspace for conversations created under that project.

## Optional Visual Bridge

The extension also contains optional visual-bridge code for local image attachment workflows. It is disabled by default and is not required for normal MCP use, the normal ChatGPT bridge, or browser-fallback sub-agents.

## Sub-agent model-selection fix

Browser-fallback sub-agents do not forward the parent's explicit ChatGPT model label into the hidden child model selector. Reasoning effort remains separate.

This avoids the previous failure:

```text
ChatGPT is not currently showing a specific model selector. Use Auto in the ChatCMD interface.
```

Regression coverage is in:

```text
chatgpt-extension/subagent-lifecycle.test.cjs
```

## PC2 acceptance checklist

A clean second PC is accepted when:

- `SETUP_PC2.cmd` completes successfully;
- installed `ChatCMD.exe` exists;
- `http://127.0.0.1:8080/api/ping` responds;
- Startup launches ChatCMD after sign-in;
- the unpacked extension loads without manifest/service-worker errors;
- ChatGPT is signed in in the same browser profile;
- a newly created per-PC MCP access profile works;
- only explicitly registered PC2 project paths are exposed;
- a normal browser-bridge turn works;
- a browser-fallback sub-agent starts while the parent model is not Auto.

## Existing prebuilt-package path

The repository also contains `packaging/custom-windows/` and `scripts/package-custom-windows.ps1` for creating an installable package from an already-built trusted EXE. That path is separate from `SETUP_PC2.cmd`, which is specifically for a fresh source clone.

## Security

Do not copy another computer's SQLite database, MCP tokens, browser cookies, ChatGPT credentials, provider/tunnel credentials, or other machine credentials into the repository or installer.

ChatCMD can expose files, Git, processes, and terminals to an AI client. Grant only the paths and tools that are actually needed.
