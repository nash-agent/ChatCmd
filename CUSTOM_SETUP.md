# ChatCMD Astra Workspace custom build

This branch contains the ChatCMD customizations currently used with Astra Workspace, including the browser-fallback sub-agent fixes and the local workspace/runtime changes.

## What is installed on a Windows PC

There are three separate pieces:

1. **ChatCMD.exe** — the local server, UI, MCP runtime, task database, workspace tools, execution tools, and sub-agent coordinator.
2. **chatgpt-extension/** — an unpacked Chromium Manifest V3 extension. This is the ChatGPT browser bridge. It is not an OpenAI API key, not a CRX, and not a separate hosted service.
3. **A per-PC MCP/plugin connection** — created after installation. Access-profile tokens, tunnel credentials, public URLs, and ChatGPT plugin/connector credentials are deliberately not copied from another PC.

The browser extension may be optional for pure MCP clients, but it is required for ChatGPT browser fallback workflows and the current ChatCMD-to-ChatGPT web integration.

## Recommended Windows install

Build or obtain a trusted `ChatCMD.exe`, then create the distributable ZIP with:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-custom-windows.ps1 `
  -BinaryPath "C:\path\to\ChatCMD.exe"
```

The resulting package contains:

- `INSTALL.cmd`
- `install.ps1`
- `uninstall.ps1`
- `README_FIRST.txt`
- `SHA256SUMS.txt`
- `payload\ChatCMD.exe`
- `payload\chatgpt-extension\...`

### What INSTALL.cmd automates

The installer:

- installs per-user under `%LOCALAPPDATA%\Programs\ChatCMD-Custom`;
- copies `ChatCMD.exe` and the stable unpacked extension directory;
- verifies the installed EXE against the packaged EXE with SHA-256;
- creates a Windows Startup shortcut;
- creates a Start Menu shortcut;
- launches ChatCMD;
- checks `http://127.0.0.1:8080/api/ping`;
- writes `NEXT_STEPS.txt`;
- copies the unpacked extension path to the clipboard.

Administrator rights are not required for the normal install path.

## Components: what is actually installed

There is only **one Chromium browser extension** in this custom build:

| Component | What it is | Install on PC2? |
| --- | --- | --- |
| `ChatCMD.exe` | Local ChatCMD server/UI/runtime | Yes; `INSTALL.cmd` handles it |
| **ChatCMD ChatGPT Bridge** (`chatgpt-extension/`) | The single unpacked Chrome/Edge/Brave extension; it contains the ChatGPT DOM bridge, sub-agent browser fallback, approvals, capture, image/visual bridge code, and related background scripts | Yes for the current ChatGPT browser workflow and reliable browser-fallback sub-agents |
| **Astra Workspace / ChatCMD plugin** | The MCP connection exposed by ChatCMD and connected inside ChatGPT; this is **not** another Chrome extension | Yes, create/connect it per PC |
| Tunnel/public HTTPS address | A route that lets web-hosted ChatGPT reach the local MCP server | Only when the ChatGPT plugin cannot reach the PC directly |

Files such as `background-visual.js`, `GlobalSubagentFallbackBridge`, or other “bridge” modules are parts of the same ChatCMD application/extension. They are **not additional browser extensions** and are not loaded separately.

### Optional Visual Bridge is not required

The extension also contains an optional **Visual Bridge** path for automatically attaching images from a loopback-only local image feeder. It is shipped disabled (`enabled:false`, `autoSend:false`) and is **not required** for MCP tools, normal ChatGPT bridging, or sub-agents.

If someone intentionally enables it, they must separately provide a local service implementing the expected `/latest` and `/v/...` image endpoints, configure a PC-specific `deviceId`, and point `origin` at that loopback service. The default config uses `http://127.0.0.1:8765` as a placeholder loopback origin; this repository does not contain or install that image-feeder server.

Other MCP hosts that support native delegation may not require the browser extension for sub-agents. In the current ChatGPT integration, when native delegation is unavailable, ChatCMD uses the **ChatCMD ChatGPT Bridge** as the sub-agent fallback, so install it if sub-agents are required.

## Register folders that ChatCMD may work in

The user can register project/workspace paths from the ChatCMD UI; no source edit or machine-specific hardcoding is required.

1. Open ChatCMD at `http://127.0.0.1:8080`.
2. In the conversation rail, find **Projects** and select the `+` (**Add project**) button.
3. Enter a display name.
4. Under **Project folder**, choose **Choose folder** and select the real folder on this PC.
5. Optionally enter the matching ChatGPT Project link if conversations should open inside a specific ChatGPT Project.
6. Save. Repeat for every independent project/root that should be available.
7. Start the conversation under that project. For that task, `workspace_roots` exposes the project as `@project`, and the runtime adds the existing project directory to that task's allowed filesystem/Git scope.

The saved path is per-PC configuration in ChatCMD's local data. It is not compiled into the executable or stored in this repository. A path from PC1 does not need to exist on PC2; choose the corresponding PC2 folder there.

Additional explicit absolute work paths supplied for a task are still subject to ChatCMD's task-scoped path/approval rules. Do not solve cross-machine setup by adding personal absolute paths to source code.

## Manual steps that remain on every new PC

These steps should not be silently cloned from another machine because they involve browser state, local paths, or credentials.

1. Open ChatCMD locally and complete any first-run local UI authentication.
2. Register the PC2 project folders using **Projects → + → Project folder → Choose folder**.
3. Open `chrome://extensions/`, `edge://extensions/`, or `brave://extensions/`.
4. Enable **Developer mode**.
5. Choose **Load unpacked** and select:
   `%LOCALAPPDATA%\Programs\ChatCMD-Custom\chatgpt-extension`
6. Confirm the extension name is **ChatCMD ChatGPT Bridge**. There is no second ChatCMD/Astra Chromium extension to load.
7. Sign in to `chatgpt.com` in that same browser profile.
8. Reload the ChatGPT tab once after loading/reloading the extension.
9. Create a new ChatCMD MCP access profile for this PC.
10. If remote ChatGPT access is required, configure this PC's public/tunnel address and verify connectivity.
11. In ChatGPT, create/connect the **Astra Workspace / ChatCMD MCP plugin** using the **new PC's** MCP endpoint/token. This plugin is separate from the Chromium extension, but it is not itself a browser extension.

Do **not** copy another PC's:

- SQLite database/settings directory;
- MCP access tokens;
- tunnel credentials;
- ChatGPT/plugin credentials;
- browser cookies or ChatGPT login state.

## Sub-agent model-selection behavior in this custom build

Browser-fallback sub-agents do not force the parent turn's explicit model label through the hidden-tab model selector.

This avoids the failure:

```text
ChatGPT is not currently showing a specific model selector. Use Auto in the ChatCMD interface.
```

A parent ChatGPT turn can therefore use a non-Auto model while a browser-fallback child still starts normally. Sub-agent reasoning effort remains separately configurable and is forwarded as `effort`.

The regression test is in:

`chatgpt-extension/subagent-lifecycle.test.cjs`

## PC2 acceptance checklist

A second Windows PC is considered verified only after all of these pass on that machine:

- [ ] `INSTALL.cmd` exits successfully.
- [ ] Installed EXE hash matches the packaged hash.
- [ ] `http://127.0.0.1:8080/api/ping` responds.
- [ ] ChatCMD survives a restart/sign-in and starts automatically.
- [ ] Unpacked extension loads without manifest/service-worker errors.
- [ ] ChatGPT is signed in in the same browser profile.
- [ ] ChatCMD can open/send a normal ChatGPT browser turn.
- [ ] The extension returns the final answer to the matching ChatCMD task.
- [ ] A new MCP access profile created on PC2 connects successfully.
- [ ] Workspace/file tools only expose the paths explicitly granted on PC2.
- [ ] A simple execution tool call works under the intended approval mode.
- [ ] A sub-agent can be started while the parent ChatGPT model is **not Auto**.
- [ ] That sub-agent reaches `running` without the “specific model selector” error.
- [ ] The child returns a durable report and the parent can read it with `agent_subagent_wait`.
- [ ] Uninstall preserves local data by default; `-RemoveData` removes it only when explicitly requested.

Until this checklist is run on PC2, the package is locally verified but not a clean-machine acceptance test.

## Source build

The upstream project already provides `scripts/build-windows.ps1` for source release builds. This custom packaging layer intentionally does not commit a prebuilt EXE into Git.

Use the upstream build script to produce a Windows binary, then pass the trusted EXE to `scripts/package-custom-windows.ps1`.

## Repository layout and upstream

Keep the original ChatCMD repository as `upstream` and this fork as `origin`.

The custom branch is intentionally separate from `main` until the PC2 clean-machine checklist passes. This avoids overwriting newer upstream ChatCMD changes with an older custom snapshot.

## Security note

ChatCMD can expose files, Git, processes, and terminals to an AI client. Use per-PC credentials, the smallest useful tool allowlist, and approval mode where appropriate. Never publish a tokenized MCP URL in screenshots, docs, issues, or commits.
