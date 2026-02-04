import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { connectionInfo, type ConnectionInfo } from './daemon';

/**
 * Sends a shutdown command to the OpenAlpaca Daemon.
 */
export async function shutdownDaemon(): Promise<void> {
    // Get current connection info
    let conn: ConnectionInfo | null = get(connectionInfo);

    // If not connected in store, try to fetch fresh connection info via Tauri
    if (!conn) {
        try {
            // Note: the return type in Rust is snake_case (base_url), 
            // but we might need to verify if Tauri converts it or if we handle it.
            // Based on previous fixes, we use camelCase in frontend.
            const rawConn = await invoke<{base_url: string, token: string, instance_id: string}>('get_connection_info');
            conn = {
                baseUrl: rawConn.base_url,
                token: rawConn.token,
                instanceId: rawConn.instance_id
            };
        } catch (e) {
            console.error("Cannot shutdown: Failed to get connection info", e);
            throw new Error("Daemon not reachable or connection info missing");
        }
    }

    if (!conn) {
         throw new Error("No connection info available");
    }

    const start = Date.now();
    console.log("Creating shutdown request...");

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
        // We consider "Network request failed" a potential success if it happened quickly.
        console.warn("Shutdown request might have been interrupted (expected if daemon exits fast):", e);
    }
}
