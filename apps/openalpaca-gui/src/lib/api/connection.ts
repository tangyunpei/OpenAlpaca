import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";

export async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}
