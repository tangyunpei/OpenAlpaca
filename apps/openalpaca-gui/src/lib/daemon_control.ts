import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { connectionInfo, disconnect, type ConnectionInfo } from './daemon';

/**
 * Sends a shutdown command to the OpenAlpaca Daemon.
 * Disconnects the WebSocket first to prevent reconnect-after-quit.
 */
export async function shutdownDaemon(): Promise<void> {
    // Get current connection info
    let conn: ConnectionInfo | null = get(connectionInfo);

    // If not connected in store, try to fetch fresh connection info via Tauri
    if (!conn) {
        try {
            conn = await invoke<ConnectionInfo>('get_connection_info');
        } catch (e) {
            console.error("Cannot shutdown: Failed to get connection info", e);
            throw new Error("Daemon not reachable or connection info missing");
        }
    }

    if (!conn) {
         throw new Error("No connection info available");
    }

    // Disconnect WS first so onclose won't trigger reconnect → respawn
    disconnect();

    try {
        const response = await fetch(`${conn.baseUrl}/v1/command`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Authorization': `Bearer ${conn.token}`
            },
            body: JSON.stringify({
                command: 'shutdown'
            })
        });

        if (!response.ok) {
            throw new Error(`Shutdown failed: ${response.status} ${response.statusText}`);
        }

        const data = await response.json();
        console.log("Shutdown command response:", data);

    } catch (e) {
        // If the daemon shuts down very quickly, the fetch might fail with a network error.
        // We consider this a potential success if the daemon exits fast.
        console.warn("Shutdown request might have been interrupted (expected if daemon exits fast):", e);
    }
}
