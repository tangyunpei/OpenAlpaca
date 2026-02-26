import { get } from 'svelte/store';
import { connectionInfo, type ConnectionInfo } from './daemon';

export interface Connector {
    id: string;
    name: string;
    status: 'active' | 'error' | 'disabled' | 'unconfigured';
    configured: boolean;
}

/**
 * Fetch status of all connectors from the daemon.
 */
export async function getConnectors(): Promise<Connector[]> {
    const conn = await ensureConnection();
    const response = await fetch(`${conn.baseUrl}/v1/connectors`, {
        headers: {
            'Authorization': `Bearer ${conn.token}`
        }
    });

    if (!response.ok) {
        throw new Error(`Failed to fetch connectors: ${response.statusText}`);
    }

    return await response.json();
}

/**
 * Perform an action on a connector (enable, disable, delete).
 */
export async function performConnectorAction(id: string, action: 'enable' | 'disable' | 'delete'): Promise<void> {
    const conn = await ensureConnection();
    const response = await fetch(`${conn.baseUrl}/v1/connectors/${id}/action`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${conn.token}`
        },
        body: JSON.stringify({ action })
    });

    if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || `Action ${action} failed: ${response.statusText}`);
    }
}

/**
 * Update connector configuration (e.g. set token).
 */
export async function configureConnector(id: string, token: string): Promise<void> {
    const conn = await ensureConnection();
    const response = await fetch(`${conn.baseUrl}/v1/connectors/${id}/config`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${conn.token}`
        },
        body: JSON.stringify({ token })
    });

    if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || `Configuration failed: ${response.statusText}`);
    }
}

/**
 * Generate a new link token for account binding.
 */
export async function generateLinkToken(): Promise<string> {
    const conn = await ensureConnection();
    const response = await fetch(`${conn.baseUrl}/v1/auth/link`, {
        method: 'POST',
        headers: {
            'Authorization': `Bearer ${conn.token}`
        }
    });

    if (!response.ok) {
        throw new Error(`Failed to generate link token: ${response.statusText}`);
    }

    const data = await response.json();
    return data.token;
}

/**
 * Get connector settings (config keys for the connector prefix).
 */
export async function getConnectorSettings(id: string): Promise<Record<string, string | null>> {
    const conn = await ensureConnection();
    const response = await fetch(`${conn.baseUrl}/v1/connectors/${id}/settings`, {
        headers: {
            'Authorization': `Bearer ${conn.token}`
        }
    });

    if (!response.ok) {
        throw new Error(`Failed to fetch settings: ${response.statusText}`);
    }

    return await response.json();
}

/**
 * Update connector settings.
 */
export async function updateConnectorSettings(id: string, settings: Record<string, string>): Promise<void> {
    const conn = await ensureConnection();
    const response = await fetch(`${conn.baseUrl}/v1/connectors/${id}/settings`, {
        method: 'PUT',
        headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${conn.token}`
        },
        body: JSON.stringify({ settings })
    });

    if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || `Settings update failed: ${response.statusText}`);
    }
}

async function ensureConnection(): Promise<ConnectionInfo> {
    const conn = get(connectionInfo);
    if (!conn) {
        throw new Error("Not connected to daemon");
    }
    return conn;
}
