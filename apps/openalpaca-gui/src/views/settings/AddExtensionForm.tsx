/**
 * `Add extension` — the GAP-24 install flow (ADR-030 §9.2 + plan Phase 8 #9).
 *
 * Two kinds, two shapes, one rule each:
 *
 *  * **A plugin is a directory.** The form takes an absolute path — picked or
 *    typed — and `Check` runs the daemon's dry run, so the manifest is on
 *    screen *before* anything is copied. Installing then **grants nothing**:
 *    the row lands `unapproved`, and the form says so rather than implying the
 *    plugin is now running.
 *  * **An MCP server is a declaration.** Writing it into your own
 *    `config/mcp.toml` is the consent, so there is no approve step — `Connect
 *    now` is the ENABLE bit, and clearing it declares the server turned off.
 *
 * The picker is a dynamic import: outside the Tauri shell (`bun run dev`, the
 * test environment) there is no dialog plugin, and the typed path is the whole
 * form either way.
 */

import { useState } from "react";

import { Button } from "@/components/ui";
import {
  useAddMcpServer,
  useInstallPlugin,
  useValidatePlugin,
} from "@/hooks/useExtensions";
import type { ManifestSummary, McpDeclaration } from "@/lib/api/types";

import { extensionErrorCopy } from "./extension-row";

type AddKind = "plugin" | "mcp";

export interface AddExtensionFormProps {
  /** Called with a sentence to toast, on success and on refusal alike. */
  onDone: (message: string) => void;
  onCancel: () => void;
}

/** Open the native directory picker, if there is one. */
export async function pickDirectory(): Promise<string | null> {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  } catch {
    // No Tauri shell (or the user has no picker): the text field is the form.
    return null;
  }
}

export function AddExtensionForm({ onDone, onCancel }: AddExtensionFormProps) {
  const [kind, setKind] = useState<AddKind>("plugin");

  return (
    <div className="flex flex-col gap-[10px] border-b border-line-hair-2 bg-sunken px-[16px] py-[12px]">
      <div
        role="radiogroup"
        aria-label="What to add"
        className="flex gap-[6px]"
      >
        {(["plugin", "mcp"] as const).map((option) => (
          <Button
            key={option}
            role="radio"
            aria-checked={kind === option}
            variant={kind === option ? "primarySm" : "secondarySm"}
            onClick={() => setKind(option)}
          >
            {option === "plugin" ? "Plugin" : "MCP server"}
          </Button>
        ))}
      </div>

      {kind === "plugin" ? (
        <PluginForm onDone={onDone} onCancel={onCancel} />
      ) : (
        <McpForm onDone={onDone} onCancel={onCancel} />
      )}
    </div>
  );
}

function PluginForm({ onDone, onCancel }: AddExtensionFormProps) {
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<{
    manifest: ManifestSummary;
    installed: boolean;
  } | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const validate = useValidatePlugin();
  const install = useInstallPlugin();
  const busy = validate.isPending || install.isPending;

  const refuse = (error: Error) => {
    setPreview(null);
    const copy = extensionErrorCopy(error.message);
    setFailure(copy);
    onDone(copy);
  };

  return (
    <div className="flex flex-col gap-[8px]">
      <span className="flex flex-wrap items-center gap-[6px]">
        <input
          aria-label="Plugin directory"
          placeholder="/Users/you/src/my-plugin"
          value={path}
          onChange={(event) => {
            setPath(event.target.value);
            setPreview(null);
            setFailure(null);
          }}
          className="min-w-[280px] flex-1 rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
        />
        <Button
          variant="secondarySm"
          disabled={busy}
          onClick={() => {
            void pickDirectory().then((picked) => {
              if (picked !== null) {
                setPath(picked);
                setPreview(null);
                setFailure(null);
              }
            });
          }}
        >
          Browse…
        </Button>
        <Button
          variant="secondarySm"
          disabled={busy || path.trim().length === 0}
          onClick={() =>
            validate.mutate(path.trim(), {
              onSuccess: (result) => {
                setFailure(null);
                setPreview(result);
              },
              onError: refuse,
            })
          }
        >
          Check
        </Button>
        <Button
          variant="primarySm"
          disabled={busy || path.trim().length === 0}
          onClick={() =>
            install.mutate(path.trim(), {
              onSuccess: (result) => {
                setFailure(null);
                onDone(
                  `${result.extension.id} installed — approve it to start it`,
                );
                onCancel();
              },
              onError: refuse,
            })
          }
        >
          Install
        </Button>
        <Button variant="secondarySm" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
      </span>

      {preview !== null && (
        <ManifestPreview
          manifest={preview.manifest}
          installed={preview.installed}
        />
      )}
      {failure !== null && (
        <span className="font-mono text-2xs-plus text-red-ink">{failure}</span>
      )}
      <span className="font-mono text-2xs-plus text-faint">
        The directory is copied in under its own name. Installing starts nothing
        — approve it afterwards to run it.
      </span>
    </div>
  );
}

/** What the dry run found — the decision `approve` will be asked to make. */
export function ManifestPreview({
  manifest,
  installed,
}: {
  manifest: ManifestSummary;
  installed: boolean;
}) {
  const contributes = Object.entries(manifest.types)
    .filter(([, on]) => on)
    .map(([name]) => name);

  return (
    <span className="flex flex-col gap-[2px] font-mono text-2xs-plus text-faint">
      <span className="text-ink">
        {manifest.name} v{manifest.version}
      </span>
      {manifest.description.length > 0 && <span>{manifest.description}</span>}
      {contributes.length > 0 && (
        <span>Contributes: {contributes.join(", ")}</span>
      )}
      <span>
        {manifest.capabilities.length > 0
          ? `Asks for: ${manifest.capabilities.join(", ")}`
          : "Declares no capabilities"}
      </span>
      {manifest.required_config_keys.length > 0 && (
        <span>
          Needs configuring: {manifest.required_config_keys.join(", ")}
        </span>
      )}
      {installed && (
        <span className="text-red-ink">
          A plugin of this name is already installed — use Update on its row.
        </span>
      )}
    </span>
  );
}

function McpForm({ onDone, onCancel }: AddExtensionFormProps) {
  const [name, setName] = useState("");
  const [transport, setTransport] = useState<"stdio" | "http">("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [url, setUrl] = useState("");
  const [bearerEnv, setBearerEnv] = useState("");
  const [connectNow, setConnectNow] = useState(true);
  const [failure, setFailure] = useState<string | null>(null);

  const add = useAddMcpServer();

  const submit = () => {
    const declaration: McpDeclaration = {
      name: name.trim(),
      transport,
      enabled: connectNow,
    };
    if (transport === "stdio") {
      declaration.command = command.trim();
      // Whitespace-separated, because that is how a command line reads; an
      // argument that needs a space belongs in `config/mcp.toml` by hand.
      const parsed = args.trim().split(/\s+/).filter(Boolean);
      if (parsed.length > 0) declaration.args = parsed;
    } else {
      declaration.url = url.trim();
      if (bearerEnv.trim().length > 0) {
        declaration.bearer_env = bearerEnv.trim();
      }
    }

    add.mutate(declaration, {
      onSuccess: (result) => {
        setFailure(null);
        onDone(`${result.extension.id} declared (${result.extension.state})`);
        onCancel();
      },
      onError: (error) => {
        const copy = extensionErrorCopy(error.message);
        setFailure(copy);
        onDone(copy);
      },
    });
  };

  const ready =
    name.trim().length > 0 &&
    (transport === "stdio" ? command.trim().length > 0 : url.trim().length > 0);

  return (
    <div className="flex flex-col gap-[8px]">
      <span className="flex flex-wrap items-center gap-[6px]">
        <Field
          label="Server name"
          value={name}
          onChange={setName}
          placeholder="github"
        />
        <select
          aria-label="Transport"
          value={transport}
          onChange={(event) =>
            setTransport(event.target.value === "http" ? "http" : "stdio")
          }
          className="rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
        >
          <option value="stdio">stdio</option>
          <option value="http">http</option>
        </select>
      </span>

      <span className="flex flex-wrap items-center gap-[6px]">
        {transport === "stdio" ? (
          <>
            <Field
              label="Command"
              value={command}
              onChange={setCommand}
              placeholder="npx"
            />
            <Field
              label="Arguments"
              value={args}
              onChange={setArgs}
              placeholder="-y @modelcontextprotocol/server-github"
            />
          </>
        ) : (
          <>
            <Field
              label="URL"
              value={url}
              onChange={setUrl}
              placeholder="https://example.com/mcp"
            />
            <Field
              label="Bearer token env var"
              value={bearerEnv}
              onChange={setBearerEnv}
              placeholder="GITHUB_TOKEN"
            />
          </>
        )}
      </span>

      <span className="flex flex-wrap items-center gap-[8px]">
        <label className="flex items-center gap-[4px] font-mono text-2xs-plus text-faint">
          <input
            type="checkbox"
            checked={connectNow}
            onChange={(event) => setConnectNow(event.target.checked)}
          />
          Connect now
        </label>
        <Button
          variant="primarySm"
          disabled={add.isPending || !ready}
          onClick={submit}
        >
          Add server
        </Button>
        <Button
          variant="secondarySm"
          disabled={add.isPending}
          onClick={onCancel}
        >
          Cancel
        </Button>
      </span>

      {failure !== null && (
        <span className="font-mono text-2xs-plus text-red-ink">{failure}</span>
      )}
      <span className="font-mono text-2xs-plus text-faint">
        The block is written into config/mcp.toml — your comments and other
        servers are left as they are. Declaring a server is the consent, so
        there is no approval step.
      </span>
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (next: string) => void;
  placeholder: string;
}) {
  return (
    <input
      aria-label={label}
      placeholder={placeholder}
      value={value}
      onChange={(event) => onChange(event.target.value)}
      className="min-w-[160px] flex-1 rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
    />
  );
}
