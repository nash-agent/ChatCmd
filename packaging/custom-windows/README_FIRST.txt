ChatCMD Astra Workspace custom Windows package
==============================================

1. Extract the ZIP.
2. Double-click INSTALL.cmd.
3. Open ChatCMD and add each allowed work folder with:
   Projects -> + -> Project folder -> Choose folder
4. In Chrome/Edge/Brave extensions:
   - enable Developer mode
   - click "Load unpacked"
   - select the extension folder printed by the installer
   - confirm the extension is "ChatCMD ChatGPT Bridge"
   - this is the ONLY Chromium extension required by this package
5. Sign in to chatgpt.com in that same browser profile and reload the ChatGPT tab.
6. Create NEW MCP/plugin/tunnel credentials for this PC.
7. Connect the Astra Workspace / ChatCMD MCP plugin in ChatGPT.
   This is an MCP/plugin connection, NOT a second browser extension.

The installer handles:
- per-user installation
- EXE SHA-256 verification
- stable unpacked extension placement
- Windows Startup shortcut
- Start Menu shortcut
- ChatCMD launch
- local /api/ping health check

Not included on purpose:
- source-PC ChatCMD database/settings
- MCP access tokens
- tunnel credentials
- ChatGPT/plugin credentials
- browser login cookies

Use per-PC credentials. See CUSTOM_SETUP.md in the source repository for the complete PC2 acceptance checklist.
