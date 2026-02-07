# OpenAlpaca CLI User Guide

`openalpaca` is the command-line interface for the OpenAlpaca system, used to manage the background service (Daemon), control connectors, and manage system configuration.

## 1. Installation & Usage

In a development environment, you can run the CLI using:

```bash
cargo run -p openalpaca -- [command]
```

After compiling and installing to your system path:

```bash
openalpaca [command]
```

---

## 2. Daemon Management

The `daemon` subcommand controls the core background service of OpenAlpaca.

- **Start**: `openalpaca daemon start`
- **Stop**: `openalpaca daemon stop`
- **Restart**: `openalpaca daemon restart`
- **Status**: `openalpaca daemon status`
- **Logs**: `openalpaca daemon tail` (View real-time background logs)

---

## 3. Connector Management

The `connector` subcommand manages the lifecycle of third-party platforms (e.g., Telegram).

- **List Status**: `openalpaca connector list`
  - Shows all drivers and their current status (`active`, `disabled`, `error`, `unconfigured`).
- **Enable**: `openalpaca connector enable <name>` (e.g., `openalpaca connector enable telegram`)
- **Disable**: `openalpaca connector disable <name>` (Stops running but preserves configuration)
- **Delete/Reset**: `openalpaca connector delete <name>`
  - 🛑 **Important**: This operation not only removes the Token configuration for the connector but also **Unlinks** all identity bindings associated with that platform.

---

## 4. Configuration (Config)

The `config` subcommand manages persistent system preferences (stored in `openalpaca.db`).

### 4.1 Interactive Configuration (TUI)

Run `openalpaca config` directly to enter the interactive menu:

- **Connectors**: Configure API Tokens for various platforms.
- **System**: Set system parameters such as log levels.
- **Reset All Config**: Perform a **Factory Reset**. This completely erases all configurations, identity bindings, memories, and logs from the database.

### 4.2 Quick Commands

- **List All Configs**: `openalpaca config list`
- **Get Specific Key**: `openalpaca config get <key>`
- **Set Config Item**: `openalpaca config set <key> <value>`
- **Reset Specific Key**: `openalpaca config reset <key>`

---

## 5. Task Management (Tasks)

The `tasks` subcommand is used to view and manage tasks within the system.

- **List Tasks**: `openalpaca tasks list`
  - Optional Filter: `--status running` (Supports `queued`, `running`, `completed`, `failed`, `cancelled`, `paused`, `active`)
  - Optional Limit: `--limit 20`
  - Output Format: `--format table` (Default) or `--format json`
- **View Task Details**: `openalpaca tasks status <task_id>`
  - Displays complete task information, including title, description, status, priority, creator, and timestamp.
  - **Visual Progress Bar**: When `progress_current`/`progress_total` exists, displays as `[████████████████████] 2/2` (Green filled blocks).
  - **Pipeline Steps**: Displays assignment info for each Agent sorted by `step_order`, formatted as `[0] completed <agent_id> (role)`.
  - **Output Preview**: Displays the first 3 lines of each Agent's `result_output` (indented, gray), with `...` for content exceeding 3 lines.
  - **Status Coloring**: `pending` is yellow, `running` is green, `failed` is red.
  - JSON Format: `--format json` outputs full JSON data (including `step_order` and `result_output`).
- **View Task Logs**: `openalpaca tasks log <task_id>`
- **Create Task**: `openalpaca tasks create [description]`
- **Control Task**: `openalpaca tasks cancel <task_id>` / `pause` / `resume`

---

## 6. Desktop GUI (`openalpaca-gui`)

OpenAlpaca includes a native desktop application built with Tauri. The GUI automatically manages the daemon as a bundled sidecar — no manual daemon setup is required.

### Running in Development

```bash
cd apps/openalpaca-gui
bun install
bunx tauri dev
```

The `tauri dev` command automatically:
1. Builds the `openalpacad` daemon binary and copies it to the sidecar directory.
2. Starts the Vite dev server for the frontend.
3. Launches the native window.

### Building for Release

```bash
cd apps/openalpaca-gui
bunx tauri build
```

This produces a distributable application bundle (`.dmg` on macOS, `.msi` on Windows, `.deb`/`.AppImage` on Linux) with the daemon binary included.

### Architecture Notes

- The GUI spawns the daemon as a detached background process on launch and connects via WebSocket (`/v1/events`).
- Clicking **Quit OpenAlpaca** in the Events panel cleanly disconnects the WebSocket and sends a shutdown command to the daemon.
- WebSocket reconnection uses exponential backoff (1s base, 30s max) with jitter.
- In release builds, the daemon binary must be co-located with the application — there is no PATH fallback.
- A Content Security Policy restricts the renderer to local-only network access (`127.0.0.1`, `localhost`).

---

## 7. System Reset

If you need to completely wipe OpenAlpaca and start over:

1.  Run `openalpaca config`.
2.  Select `🗑️ Reset All Config`.
3.  Type `yes` to confirm.

This safely clears the database, including your bound Telegram account information and AI memories.
