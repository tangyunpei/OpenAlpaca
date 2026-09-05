/**
 * Settings → Extensions (ADR-030 §9.2; replaces the Plugins section).
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
 * Every field rendered here is served by `GET /v1/extensions` on day one, so
 * the section ships fully backed — no decorative disabled toggle anywhere. The
 * one `GapNote` is GAP-24: installing an extension is still a directory copy
 * or a hand-written `[servers.<name>]` block.
 *
 * There is **no per-tool switch** here or in Tools: ENABLE is one toggle per
 * MCP server and per plugin, ALLOW is per-agent capability (S1).
 */

import { useState } from "react";

import { Button, Tag } from "@/components/ui";
import {
  useExtensionInstall,
  useExtensions,
  useExtensionVerb,
  useRemoveExtension,
  useSetExtensionConfig,
} from "@/hooks/useExtensions";
import type { ExtensionRow, ExtensionVerb } from "@/lib/api/types";
import { gapDetail } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import {
  extensionErrorCopy,
  orderExtensions,
  type ExtensionRowView,
} from "./extension-row";
import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

const rowKey = (row: ExtensionRow) => `${row.kind}:${row.id}`;

export function ExtensionsSection() {
  const extensions = useExtensions();
  const verb = useExtensionVerb();
  const remove = useRemoveExtension();
  const setConfig = useSetExtensionConfig();
  const install = useExtensionInstall();
  const showToast = useUiStore((s) => s.showToast);

  /** Row-level copy for the last refusal, keyed by row (§8's flat envelope). */
  const [failure, setFailure] = useState<{ key: string; copy: string } | null>(
    null,
  );
  const [configuring, setConfiguring] = useState<string | null>(null);
  const [showDisabled, setShowDisabled] = useState(false);

  const installNote = install.available ? null : gapDetail(install);
  const ordered = orderExtensions(extensions.data ?? []);
  const live = ordered.filter((entry) => entry.view.rank !== 2);
  const off = ordered.filter((entry) => entry.view.rank === 2);
  const busy = verb.isPending || remove.isPending;

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
            {view.menu.includes("reload") && (
              <RowMenu
                label={row.id}
                disabled={busy}
                onReload={() => run(row, "reload", "reloaded")}
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
      <ListCard
        addLabel="Add extension"
        onAdd={() =>
          showToast(installNote ?? "Adding an extension is not wired up yet")
        }
      >
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

      {installNote !== null && <GapNote>{installNote}.</GapNote>}
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
  onConfigure,
}: {
  row: ExtensionRow;
  view: ExtensionRowView;
  failure: string | null;
  configuring: boolean;
  onConfigure: (entries: Array<[string, string]>) => void;
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
    !configuring
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

/** Reload lives here because nothing is wrong: it is not a primary control. */
function RowMenu({
  label,
  disabled,
  onReload,
}: {
  label: string;
  disabled: boolean;
  onReload: () => void;
}) {
  const [open, setOpen] = useState(false);

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
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onReload();
            }}
            className="cursor-pointer border-none bg-transparent px-[12px] py-[5px] text-left text-sm-plus text-ink hover:bg-muted"
          >
            Reload
          </button>
        </span>
      )}
    </span>
  );
}
