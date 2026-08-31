# DevTools Hub

DevTools Hub is a native cross-platform developer productivity tool prototype built with Rust, Slint, SQLite and Tokio.

## Phase 1 Scope

- Slint main search window
- Global hotkey window activation
- SQLite initialization and migrations
- Clipboard polling, persistence and FTS search
- Tokio background tasks communicating with the Slint UI thread
- Built-in JSON format/minify/validate tools
- Built-in Base64 encode/decode tools
- SQLite-backed settings
- Simplified Chinese by default, with English UI labels reserved
- Dark and light themes
- Configurable global hotkey, applied after restart
- Tray menu with show and quit actions
- Left-click tray icon to show the main window
- Tools open in independent resizable windows with maximize and always-on-top controls
- Clipboard history is available as a tool and records the active window title when possible
- Middle-click quick action window for selected text workflows

## Run

```powershell
cargo run -p devtools-app
```

Default hotkey:

- Windows/Linux: `Alt + Space`
- macOS: `Option + Space`

The app stores data in the platform application data directory under `DevToolsHub`.

The middle-click quick action feature uses a best-effort selected-text capture strategy:
it briefly sends the system copy shortcut, reads the clipboard, then restores the previous clipboard text.
On macOS/Linux this may require accessibility/input permissions or platform-specific follow-up work.
