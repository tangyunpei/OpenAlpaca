# OpenAlpaca GUI User Guide

This document explains the interface functions and interactive operations of the official OpenAlpaca GUI client.

---

## 1. Interface Overview

The GUI interface is divided into four main function tabs:

- **Event Log**: Real-time monitoring of system status, connection heartbeats, and background logs.
- **Connectors**: Management of third-party platform access (e.g., Telegram), configuration, and identity binding.
- **Tasks**: View and manage the lifecycle of tasks, including progress tracking and status control.
- **Agents**: View the list of registered SubAgents, including skills, metrics, and LLM configurations.

---

## 2. Status Indicators

The pill-shaped status indicator in the top right corner reflects the connection status between the GUI and the background Daemon:

- ⚫ **Disconnected**: Not connected. Usually indicates the Daemon is not started or network interruption.
- 🟡 **Connecting**: Attempting to establish a connection.
- 🟢 **Connected**: Connected. A breathing light (Dot) indicates a live link.
- 🔴 **Error**: Connection error. A red banner will appear below explaining the specific reason.

---

## 3. Event Log

This tab is used to view `ServerEvent` pushed from the Daemon:

- **Reconnect**: Manually initiate a WebSocket connection.
- **Clear**: Clears only the currently displayed list on the frontend, does not delete database records.
- **Quit**: Dangerous operation button (e.g., "Quit OpenAlpaca") will directly close the background Daemon process.

### 3.1 Event Types

The event list displays `ServerEvent` pushed from the Daemon in real-time. Each event type has a specific icon and color identifier:

| Icon | Type Code (Type)   | Description                                                                                                           |
| :--: | ------------------ | --------------------------------------------------------------------------------------------------------------------- |
|  💓  | `heartbeat`        | **Heartbeat**. Sent periodically by Daemon (default 5s) to prove service liveness. Displayed with a green background. |
|  📨  | `wake`             | **Wake Event**. From Wake modules (e.g., Cron timer triggers, file monitoring changes).                               |
|  ⚡  | `command_received` | **Command Received**. Daemon received a control command from an external API. Displayed with a red background.        |
|  📝  | `log`              | **System Log**. Info/Warn/Error logs generated internally by the Daemon. Displayed with a blue background.            |
|  🤖  | `agent_status`     | **Agent Status**. Status changes for Agent start, stop, or error. Synchronizes with the Agents panel.                 |
|  📋  | `task_status`      | **Task Status**. Updates on task progress or execution results. Synchronizes with the Tasks panel.                    |
|  🔌  | `connector_status` | **Connector Status**. Status changes for connector enable/disable/error.                                              |

> **Note**: The list automatically retains the latest 100 events, and old events are automatically scrolled off.

---

## 4. Connector Management (Connectors)

This tab is the core area for connecting external platforms.

### 4.1 Connector List

Displays all platforms supported by the system (e.g., Telegram) and their current status:

- **Active**: Configured and running.
- **Disabled**: Configured but manually turned off.
- **Unconfigured**: Missing necessary information such as Token.
- **Error**: Startup failed (e.g., invalid Token).

### 4.2 Configuration & Operations

Each connector card provides the following operations:

- **⚙️ Configure**: Click to open the modal, enter and save the Bot Token (e.g., Telegram API Token).
- **Toggle**: Quickly enable or disable the connector.
- **✕ Delete**: Performs a **"Partial Reset"**. This stops the connector, clears its configuration, and **automatically unlinks** all identity information associated with that platform.

---

## 5. Task Panel (Tasks)

This tab is used to view and manage all tasks in the system. Tasks are created via Orchestrator and executed by SubAgents.

### 5.1 Task List

The top provides the following controls:

- **Refresh**: Manually re-fetch the task list from the REST API.
- **Active / Completed**: Switch the view between active tasks and completed tasks.

Each task is displayed as a card containing:

- **Title**: Task name.
- **Status Badge**: Color-coded status identifier.
- **Progress Bar**: Displayed when both `progress_current` and `progress_total` are not empty.
- **Meta Info**: Task ID (first 8 characters) and creation time.

### 5.2 Task Status Description

| Status      | Description                           |
| ----------- | ------------------------------------- |
| `queued`    | Created, waiting for Agent execution. |
| `running`   | Currently executing.                  |
| `paused`    | Paused, can be resumed.               |
| `completed` | Execution successfully completed.     |
| `failed`    | Execution failed.                     |
| `cancelled` | Cancelled by user.                    |

### 5.3 Task Detail

Clicking a task card opens the detail modal, containing:

- **Full Info**: ID, status, priority, creator, source Lane, timestamp.
- **Progress Bar**: When `progress_current` and `progress_total` exist and `progress_total > 0`, displays an inline progress bar and `current/total` text.
- **Description**: Detailed description of the task (if any).
- **Result**: Summary of results after task completion (if any).
- **Pipeline Assignments**: Displays all Agents participating in this task, including columns:
  - **Step**: Execution order (`step_order`), displays `-` if no order.
  - **Agent**: Agent ID (first 8 characters).
  - **Role**: Agent's role in this task.
  - **Status**: Assignment status badge (pending/running/completed/failed).
  - **Output**: Collapsible Agent output. When `result_output` is non-empty, displays a "View" button, clicking expands/collapses output content (`<pre>` block, max height 200px scrollable). Empty output displays `-`.

### 5.4 Task Operations

The bottom of the detail modal provides operation buttons based on the current status:

- **Cancel**: Available in `queued`, `running`, or `paused` states.
- **Pause**: Available only in `running` state.
- **Resume**: Available only in `paused` state.

### 5.5 Real-time Updates

The task panel updates in real-time via WebSocket `task_status` events. When background task status changes, the list and details refresh automatically without manual intervention.

---

## 6. Agent Panel (Agents)

This tab displays all registered SubAgents in the system. Agent configuration is loaded from `config/agents/*.toml` files.

### 6.1 Agent List

- **Refresh**: Manually re-fetch the Agent list from the REST API.

Each Agent is displayed as a card containing:

- **Icon**: Agent custom icon or default robot icon.
- **Name/Desc**: Name and description.
- **Status Light**: Green (idle), Orange (busy), Blue (waiting), Red (error), Gray (offline).
- **Current Task**: If the Agent is executing a task, displays the task ID.
- **Skill Tags**: List of skills parsed from `skills_json` (displays up to 4).

### 6.2 Agent Detail

Clicking an Agent card opens the detail modal containing the following blocks:

- **Basic Info**: ID, status, current task, creation time.
- **Skills**: Complete list of skill tags.
- **Metrics**: Number of completed tasks, failed tasks, success rate, average runtime, and success rate progress bar.
- **LLM Config**: Model used by the Agent (model), fallback models (fallback_models), parsed from `llm_config_json`.
- **Constraints**: Allowed/forbidden model lists, max cost per single task, parsed from `constraints_json`.

### 6.3 Agent Operations

The bottom of the detail modal provides operation buttons based on the current status:

- **Pause**: Available in `idle` or `busy` states. After pausing, Agent enters `waiting` state.
- **Resume**: Available only in `waiting` state. After resuming, Agent returns to `idle` state.

### 6.4 Real-time Updates

The Agent panel updates in real-time via WebSocket `agent_status` events. When Agent status changes (e.g., from idle to busy), the list automatically reflects the latest status.

---

## 7. Identity Binding & Unbinding

OpenAlpaca uses a unified identity system to link accounts from different platforms (e.g., your Telegram ID) to the same OpenAlpaca user.

### 7.1 How to Bind

1.  In the **Connectors** tab, click **Generate Bind Token**.
2.  Copy the generated 8-character uppercase code (valid for 5 minutes).
3.  In the Telegram bot chat, send the command: `/link <YOUR_CODE>`.
4.  After successful binding, the bot will recognize your OpenAlpaca identity and allow interaction.

### 7.2 How to Unbind

There are two ways to remove identity leaks or switch accounts:

- **Method A (Recommended)**: Send `/unlink` or `/unbind` command directly in Telegram.
- **Method B (Force)**: Click the **Delete (✕)** button for the connector in the GUI Connectors tab. This erases the binding relationship for that platform from the OpenAlpaca database.

---

## 8. FAQ

**Q: Why does the generated bind token show Failed?**
A: Please ensure the background Daemon is connected. Bind token generation relies on Daemon access to the database.

**Q: What if the connector shows "Error"?**
A: Click the ⚙️ icon to re-check and confirm your Bot Token is correct, then save and enable again.

**Q: Tasks panel is empty?**
A: Tasks need to be created via Orchestrator or REST API (`POST /v1/tasks`). Confirm Daemon is connected and tasks have been created.

**Q: Agents panel is empty?**
A: Agent configuration is loaded from `config/agents/*.toml`. Confirm valid TOML configuration files exist in that directory and were successfully loaded when Daemon started.

**Q: Task/Agent status not updating in real-time?**
A: Confirm `heartbeat` events are received in the Event Log, indicating WebSocket connection is normal. If disconnected, click the Reconnect button in the Event Log tab.
