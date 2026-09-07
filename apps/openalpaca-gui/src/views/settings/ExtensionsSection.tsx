/**
 * Settings → Extensions (ADR-030 §9.2; replaces the Plugins section), with the
 * GAP-24 install flow.
 *
 * One list over both kinds — MCP servers and plugins — each row carrying a
 * `kind` chip, because the two are governed by the same one-bit ENABLE axis
 * and an owner asking "what is turned on?" should not have to know which
 * mechanism a thing arrived through.
 *
 * The section asks for `?include_orphaned=true`: the API hides orphans by
 * default so scripts and `openalpaca ext list` see only real extensions, and
 * this page is exactly where an owner needs to see one and Remove it.
 *
 * Every field rendered here is served by `GET /v1/extensions`, so the section
 * ships fully backed — no decorative disabled toggle anywhere, and no gap note
 * left: `Add extension` installs a plugin from a directory or declares an MCP
 * server, and each row can be updated and uninstalled.
 *
 * The three GAP-24 verbs stay out of the primary controls. `Update` and
 * `Uninstall` live in the row's overflow menu beside `Reload`, because they
 * change what is *installed* rather than whether it is on, and both open an
 * inline strip that has to be confirmed — an uninstall moves a directory, and
 * that should take two deliberate clicks.
 *
 * There is **no per-tool switch** here or in Tools: ENABLE is one toggle per
 * MCP server and per plugin, ALLOW is per-agent capability (S1).
 */

import { useState } from "react";

import { Button, Tag } from "@/components/ui";
import {
  useExtensions,
  useExtensionVerb,
  useRemoveExtension,
  useSetExtensionConfig,
  useUninstallExtension,
  useUpdatePlugin,
} from "@/hooks/useExtensions";
import type { ExtensionRow, ExtensionVerb } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { AddExtensionForm, pickDirectory } from "./AddExtensionForm";
import {
  extensionErrorCopy,
  orderExtensions,
  type ExtensionMenuItem,
  type ExtensionRowView,
} from "./extension-row";
import { ListCard, ListRow, ListState, Toggle } from "./primitives";

const rowKey = (row: ExtensionRow) => `${row.kind}:${row.id}`;

export function ExtensionsSection() {
  const extensions = useExtensions();
  const verb = useExtensionVerb();
  const remove = useRemoveExtension();
  const setConfig = useSetExtensionConfig();
  const update = useUpdatePlugin();
  const uninstall = useUninstallExtension();
  const showToast = useUiStore((s) => s.showToast);

  /** Row-level copy for the last refusal, keyed by row (§8's flat envelope). */
  const [failure, setFailure] = useState<{ key: string; copy: string } | null>(
    null,
  );
  const [configuring, setConfiguring] = useState<string | null>(null);
  const [showDisabled, setShowDisabled] = useState(false);
  const [adding, setAdding] = useState(false);
  /** The row whose inline Update or Uninstall strip is open, if any. */
  const [pending, setPending] = useState<{
    key: string;
    item: ExtensionMenuItem;
  } | null>(null);

  const ordered = orderExtensions(extensions.data ?? []);
  const live = ordered.filter((entry) => entry.view.rank !== 2);
  const off = ordered.filter((entry) => entry.view.rank === 2);
  const busy =
    verb.isPending ||
    remove.isPending ||
    update.isPending ||
    uninstall.isPending;

  const onError = (row: ExtensionRow, error: Error) => {
    const copy = extensionErrorCopy(error.message);
    setFailure({ key: rowKey(row), copy });
    showToast(`${row.id} — ${copy}`);
  };

  const run = (row: ExtensionRow, action: ExtensionVerb, said: string) =>
    verb.mutate(
      { kind: row.kind, id: row.id, verb: action },
      {
        onSuccess: (updated) => {
          setFailure(null);
          const warnings = updated.warnings ?? [];
          showToast(
            warnings.length > 0
              ? `${row.id} ${said} — ${warnings.join("; ")}`
              : `${row.id} ${said}`,
          );
        },
        onError: (error) => onError(row, error),
      },
    );

  const renderRow = ({
    row,
    view,
  }: {
    row: ExtensionRow;
    view: ExtensionRowView;
  }) => {
    const key = rowKey(row);

    return (
      <ListRow
        key={key}
        name={row.id}
        tags={
          <>
            <Tag value={row.kind === "mcp" ? "MCP" : "Plugin"} />
            <Tag value={view.tag} tone={view.tone} />
          </>
        }
        description={view.description}
        chips={rowDetail({
          row,
          view,
          failure: failure?.key === key ? failure.copy : null,
          configuring: configuring === key,
          pending: pending?.key === key ? pending.item : null,
          onConfigure: (entries) => {
            for (const [configKey, value] of entries) {
              setConfig.mutate(
                { id: row.id, key: configKey, value },
                {
                  onSuccess: () => showToast(`${row.id} — ${configKey} set`),
                  onError: (error) => onError(row, error),
                },
              );
            }
            setConfiguring(null);
          },
          onUpdate: (path) => {
            update.mutate(
              { id: row.id, path },
              {
                onSuccess: (result) => {
                  setFailure(null);
                  setPending(null);
                  showToast(
                    result.consent_reset === true
                      ? `${row.id} updated — it now also asks for ${(result.added_capabilities ?? []).join(", ")}, so approve it again`
                      : `${row.id} updated to v${result.extension.version ?? "?"}`,
                  );
                },
                onError: (error) => onError(row, error),
              },
            );
          },
          onUninstall: (keepData) => {
            uninstall.mutate(
              { kind: row.kind, id: row.id, keepData },
              {
                onSuccess: (result) => {
                  setFailure(null);
                  setPending(null);
                  showToast(
                    result.trashed === null
                      ? `${row.id} uninstalled`
                      : `${row.id} uninstalled — kept at ${result.trashed}`,
                  );
                },
                onError: (error) => onError(row, error),
              },
            );
          },
          onCancelPending: () => setPending(null),
        })}
        meta={row.version === null ? undefined : `v${row.version}`}
        control={
          <span className="flex shrink-0 items-center gap-[6px]">
            {view.actions.includes("approve") && (
              <Button
                variant="primarySm"
                disabled={busy}
                onClick={() => run(row, "approve", "approved")}
              >
                Approve
              </Button>
            )}
            {view.actions.includes("deny") && (
              <Button
                variant="dangerGhost"
                disabled={busy}
                onClick={() => run(row, "deny", "denied")}
              >
                Deny
              </Button>
            )}
            {view.actions.includes("retry") && (
              <Button
                variant="secondarySm"
                disabled={busy}
                onClick={() => run(row, "reload", "reloaded")}
              >
                Retry
              </Button>
            )}
            {view.actions.includes("configure") && (
              <Button
                variant="secondarySm"
                disabled={setConfig.isPending}
                onClick={() => setConfiguring(configuring === key ? null : key)}
              >
                Configure
              </Button>
            )}
            {view.actions.includes("remove") && (
              <Button
                variant="dangerGhost"
                disabled={busy}
                onClick={() =>
                  remove.mutate(row.id, {
                    onSuccess: () => {
                      setFailure(null);
                      showToast(`${row.id} removed`);
                    },
                    onError: (error) => onError(row, error),
                  })
                }
              >
                Remove
              </Button>
            )}
            {view.menu.length > 0 && (
              <RowMenu
                label={row.id}
                items={view.menu}
                disabled={busy}
                onPick={(item) => {
                  if (item === "reload") run(row, "reload", "reloaded");
                  else setPending({ key, item });
                }}
              />
            )}
            {view.control === "toggle" && (
              <Toggle
                checked={view.toggleChecked}
                label={`Enable ${row.id}`}
                disabled={view.toggleDisabled || busy}
                disabledReason={view.disabledReason}
                onChange={(next) =>
                  run(
                    row,
                    next ? "enable" : "disable",
                    next ? "enabled" : "disabled",
                  )
                }
              />
            )}
          </span>
        }
      />
    );
  };

  return (
    <>
      <ListCard addLabel="Add extension" onAdd={() => setAdding(!adding)}>
        {adding && (
          <AddExtensionForm
            onDone={showToast}
            onCancel={() => setAdding(false)}
          />
        )}
        <ListState
          pending={extensions.isPending}
          error={extensions.error}
          empty={ordered.length === 0}
          emptyCopy="No MCP servers or plugins are installed."
        >
          {live.map(renderRow)}
          {off.length > 0 && (
            <>
              <button
                type="button"
                aria-expanded={showDisabled}
                onClick={() => setShowDisabled(!showDisabled)}
                className="flex w-full cursor-pointer items-center gap-[6px] border-none bg-sunken px-[16px] py-[9px] text-left font-mono text-2xs-plus text-muted-fg hover:text-ink"
              >
                <span aria-hidden>{showDisabled ? "▾" : "▸"}</span>
                {off.length} turned off
              </button>
              {showDisabled && off.map(renderRow)}
            </>
          )}
        </ListState>
      </ListCard>
    </>
  );
}

/**
 * The row's second line: where the bit lives, what the load could not publish,
 * the authorization link, and the last refusal.
 *
 * A plain function, not a component, so a row with nothing to add renders no
 * empty chip strip at all.
 */
function rowDetail({
  row,
  view,
  failure,
  configuring,
  pending,
  onConfigure,
  onUpdate,
  onUninstall,
  onCancelPending,
}: {
  row: ExtensionRow;
  view: ExtensionRowView;
  failure: string | null;
  configuring: boolean;
  /** The GAP-24 strip this row has open, if any. */
  pending: ExtensionMenuItem | null;
  onConfigure: (entries: Array<[string, string]>) => void;
  onUpdate: (path: string) => void;
  onUninstall: (keepData: boolean) => void;
  onCancelPending: () => void;
}): React.ReactNode | undefined {
  const notes: string[] = [];
  if (row.withdrawn_by_server.length > 0) {
    notes.push(
      `withdrawn by the server: ${row.withdrawn_by_server.join(", ")}`,
    );
  }
  if (row.skipped_tools.length > 0) {
    notes.push(
      `name already served elsewhere: ${row.skipped_tools.join(", ")}`,
    );
  }

  if (
    view.secondary === null &&
    notes.length === 0 &&
    failure === null &&
    view.authorizeUrl === null &&
    !configuring &&
    pending === null
  ) {
    return undefined;
  }

  return (
    <span className="flex w-full flex-col gap-[4px]">
      {view.secondary !== null && (
        <span className="font-mono text-2xs-plus text-faint">
          {view.secondary}
        </span>
      )}
      {notes.map((note) => (
        <span key={note} className="font-mono text-2xs-plus text-faint">
          {note}
        </span>
      ))}
      {view.authorizeUrl !== null && (
        <a
          href={view.authorizeUrl}
          target="_blank"
          rel="noreferrer noopener"
          className="font-mono text-2xs-plus text-blue underline"
        >
          Authorize at {view.authorizeUrl}
        </a>
      )}
      {configuring && (
        <ConfigForm keys={row.missing_config_keys} onSubmit={onConfigure} />
      )}
      {pending === "update" && (
        <UpdateForm
          id={row.id}
          onSubmit={onUpdate}
          onCancel={onCancelPending}
        />
      )}
      {pending === "uninstall" && (
        <UninstallForm
          row={row}
          onSubmit={onUninstall}
          onCancel={onCancelPending}
        />
      )}
      {failure !== null && (
        <span className="font-mono text-2xs-plus text-red-ink">{failure}</span>
      )}
    </span>
  );
}

/**
 * One input per missing key, posted one call at a time — the route's shape.
 *
 * Setting the last missing key starts the plugin without a second call: the
 * daemon invokes the `enable` verb itself when the row is `Failed{NeedsConfig}`
 * with the bit set and consent recorded (§8).
 */
function ConfigForm({
  keys,
  onSubmit,
}: {
  keys: string[];
  onSubmit: (entries: Array<[string, string]>) => void;
}) {
  const [values, setValues] = useState<Record<string, string>>({});

  return (
    <span className="flex flex-wrap items-center gap-[6px]">
      {keys.map((key) => (
        <input
          key={key}
          aria-label={key}
          placeholder={key}
          value={values[key] ?? ""}
          onChange={(event) =>
            setValues({ ...values, [key]: event.target.value })
          }
          className="rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
        />
      ))}
      <Button
        variant="primarySm"
        onClick={() =>
          onSubmit(
            Object.entries(values).filter(([, value]) => value.length > 0),
          )
        }
      >
        Save
      </Button>
    </span>
  );
}

/**
 * Replace an installed plugin’s tree. The path is asked for inline rather than
 * in a dialog, because it is the same thing the install form takes and the row
 * is where the owner already is.
 */
function UpdateForm({
  id,
  onSubmit,
  onCancel,
}: {
  id: string;
  onSubmit: (path: string) => void;
  onCancel: () => void;
}) {
  const [path, setPath] = useState("");

  return (
    <span className="flex flex-wrap items-center gap-[6px]">
      <input
        aria-label={`Replacement directory for ${id}`}
        placeholder="/Users/you/src/my-plugin"
        value={path}
        onChange={(event) => setPath(event.target.value)}
        className="min-w-[240px] flex-1 rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
      />
      <Button
        variant="secondarySm"
        onClick={() => {
          void pickDirectory().then((picked) => {
            if (picked !== null) setPath(picked);
          });
        }}
      >
        Browse…
      </Button>
      <Button
        variant="primarySm"
        disabled={path.trim().length === 0}
        onClick={() => onSubmit(path.trim())}
      >
        Update
      </Button>
      <Button variant="secondarySm" onClick={onCancel}>
        Cancel
      </Button>
    </span>
  );
}

/**
 * The uninstall confirmation. It says what will happen in the words that are
 * true: nothing is deleted, the directory is moved to `plugins/.trash/`, and
 * the plugin’s own data is kept unless the box is cleared.
 */
function UninstallForm({
  row,
  onSubmit,
  onCancel,
}: {
  row: ExtensionRow;
  onSubmit: (keepData: boolean) => void;
  onCancel: () => void;
}) {
  const [keepData, setKeepData] = useState(true);

  return (
    <span className="flex flex-wrap items-center gap-[8px]">
      <span className="font-mono text-2xs-plus text-faint">
        {row.kind === "plugin"
          ? "The directory is moved to plugins/.trash/, not deleted."
          : "The [servers] block is removed from config/mcp.toml."}
      </span>
      {row.kind === "plugin" && (
        <label className="flex items-center gap-[4px] font-mono text-2xs-plus text-faint">
          <input
            type="checkbox"
            checked={keepData}
            onChange={(event) => setKeepData(event.target.checked)}
          />
          Keep its data
        </label>
      )}
      <Button
        variant="dangerGhost"
        aria-label={`Confirm uninstalling ${row.id}`}
        onClick={() => onSubmit(keepData)}
      >
        Uninstall
      </Button>
      <Button variant="secondarySm" onClick={onCancel}>
        Cancel
      </Button>
    </span>
  );
}

/**
 * Reload, Update and Uninstall live here because none of them is a primary
 * control: nothing is wrong, and the last two change what is installed.
 */
function RowMenu({
  label,
  items,
  disabled,
  onPick,
}: {
  label: string;
  items: ExtensionMenuItem[];
  disabled: boolean;
  onPick: (item: ExtensionMenuItem) => void;
}) {
  const [open, setOpen] = useState(false);
  const copy: Record<ExtensionMenuItem, string> = {
    reload: "Reload",
    update: "Update…",
    uninstall: "Uninstall…",
  };

  return (
    <span className="relative">
      <Button
        variant="iconGlyph"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`More actions for ${label}`}
        disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        ⋯
      </Button>
      {open && (
        <span
          role="menu"
          className="absolute right-0 z-10 mt-[4px] flex flex-col rounded-md border border-line bg-raised py-[4px] shadow-lg"
        >
          {items.map((item) => (
            <button
              key={item}
              type="button"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onPick(item);
              }}
              className="cursor-pointer border-none bg-transparent px-[12px] py-[5px] text-left text-sm-plus text-ink hover:bg-muted"
            >
              {copy[item]}
            </button>
          ))}
        </span>
      )}
    </span>
  );
}
