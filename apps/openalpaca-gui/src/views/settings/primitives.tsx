/**
 * The Settings component set (DESIGN_SPEC §3.32).
 *
 * Every measurement here is transcribed from §3.32; nothing is rounded to a
 * Tailwind default, because the fractional sizes are the design.
 *
 * `Toggle` is the one control the export leaves inert ("Settings toggles …
 * display-only in the mock", §5.6). Here it is a real `role="switch"`, and each
 * section decides whether it can be driven: the connector and plugin toggles
 * call the daemon, the provider and agent-template toggles cannot (GAP-15,
 * GAP-20) and are rendered disabled beside the note that says why.
 */

import { Button, Eyebrow, LogTag } from "@/components/ui";
import { cn } from "@/lib/cn";

// ── Section navigation ──────────────────────────────────────────────────────

export interface SectionNavItemProps {
  label: string;
  active: boolean;
  count?: number;
  onSelect: () => void;
}

export function SectionNavItem({
  label,
  active,
  count,
  onSelect,
}: SectionNavItemProps) {
  return (
    <button
      type="button"
      aria-current={active ? "page" : undefined}
      onClick={onSelect}
      className={cn(
        "flex w-full cursor-pointer items-center gap-[8px] rounded-lg border-none px-[9px] py-[7px] text-left text-base-plus leading-[normal]",
        "transition-[background-color,color] duration-[120ms]",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
        active
          ? "bg-rail font-medium text-ink"
          : "bg-transparent font-normal text-secondary hover:bg-muted-2",
      )}
    >
      <span className="flex-1">{label}</span>
      {count !== undefined && (
        <span className="font-mono text-xs opacity-55">{count}</span>
      )}
    </button>
  );
}

// ── Page head ───────────────────────────────────────────────────────────────

export function PageHead({ title, blurb }: { title: string; blurb: string }) {
  return (
    <>
      <h2 className="m-0 mb-[4px] text-3xl font-semibold tracking-tightest text-ink">
        {title}
      </h2>
      <p className="m-0 mb-[22px] text-md leading-[1.6] text-tertiary">
        {blurb}
      </p>
    </>
  );
}

/** The muted line that names a missing route, under whatever it explains. */
export function GapNote({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-[8px] mb-0 font-mono text-2xs-plus leading-[1.6] text-faint">
      {children}
    </p>
  );
}

// ── Cards ───────────────────────────────────────────────────────────────────

export function Card({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section
      className={cn(
        "rounded-3xl border border-line bg-raised px-[18px] py-[16px]",
        className,
      )}
    >
      {children}
    </section>
  );
}

export interface StatusCardProps {
  ok: boolean;
  title: string;
  /** Right-aligned mono meta — uptime in the design. */
  meta?: React.ReactNode;
  cells: readonly { label: string; value: React.ReactNode }[];
  children?: React.ReactNode;
}

export function StatusCard({
  ok,
  title,
  meta,
  cells,
  children,
}: StatusCardProps) {
  return (
    <Card>
      <div className="flex items-center gap-[10px]">
        <span
          role="img"
          aria-label={ok ? "connected" : "disconnected"}
          className={cn(
            "block h-[8px] w-[8px] shrink-0 rounded-full",
            ok ? "bg-green" : "bg-red",
          )}
        />
        <span className="text-md-plus font-semibold text-ink">{title}</span>
        {meta !== undefined && (
          <span className="ml-auto font-mono text-xs-plus text-muted-fg">
            {meta}
          </span>
        )}
      </div>

      <dl className="mt-[16px] grid grid-cols-3 gap-[14px]">
        {cells.map((cell) => (
          <div key={cell.label}>
            <dt>
              <Eyebrow tracking="narrow" tone="faint" className="mb-[4px]">
                {cell.label}
              </Eyebrow>
            </dt>
            <dd className="m-0 font-mono text-base text-ink">{cell.value}</dd>
          </div>
        ))}
      </dl>

      {children}
    </Card>
  );
}

export interface StatCardProps {
  title: string;
  stats: readonly { label: string; value: string }[];
  children?: React.ReactNode;
}

export function StatCard({ title, stats, children }: StatCardProps) {
  return (
    <Card>
      <div className="mb-[12px] text-md-plus font-semibold text-ink">
        {title}
      </div>
      <div className="flex gap-[26px]">
        {stats.map((stat) => (
          <div key={stat.label}>
            <div className="text-3xl font-semibold text-ink">{stat.value}</div>
            <div className="mt-[2px] font-mono text-2xs-plus text-muted-fg">
              {stat.label}
            </div>
          </div>
        ))}
      </div>
      {children}
    </Card>
  );
}

// ── List card ───────────────────────────────────────────────────────────────

export interface ListCardProps {
  /** The design's add bar; omit for a section with no add action. */
  addLabel?: string;
  onAdd?: () => void;
  children: React.ReactNode;
}

export function ListCard({ addLabel, onAdd, children }: ListCardProps) {
  return (
    <div className="overflow-hidden rounded-3xl border border-line bg-raised">
      {addLabel !== undefined && (
        <div className="flex justify-end border-b border-line-hair-2 bg-sunken px-[14px] py-[10px]">
          <Button variant="primarySm" onClick={onAdd}>
            {addLabel}
          </Button>
        </div>
      )}
      {children}
    </div>
  );
}

export interface ListRowProps {
  name: React.ReactNode;
  /** Tags sit beside the name. */
  tags?: React.ReactNode;
  description?: React.ReactNode;
  /** Model chips, or any wrapped control row under the description. */
  chips?: React.ReactNode;
  /** Mono meta pinned right of the description column. */
  meta?: React.ReactNode;
  /** The trailing control — a `Toggle`, buttons, or nothing. */
  control?: React.ReactNode;
}

export function ListRow({
  name,
  tags,
  description,
  chips,
  meta,
  control,
}: ListRowProps) {
  return (
    <div className="flex items-center gap-[12px] border-b border-line-hair-2 px-[16px] py-[13px]">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-[8px]">
          <span className="truncate text-md font-medium text-ink">{name}</span>
          {tags}
        </div>
        {description !== undefined && (
          <div className="mt-[3px] text-base leading-[1.5] text-tertiary">
            {description}
          </div>
        )}
        {chips !== undefined && (
          <div className="mt-[8px] flex flex-wrap gap-[5px]">{chips}</div>
        )}
      </div>
      {meta !== undefined && (
        <span className="shrink-0 font-mono text-xs text-faint">{meta}</span>
      )}
      {control}
    </div>
  );
}

/** §3.32's log row — mono message, unlike the work detail's sans one. */
export function LogRow({
  tag,
  text,
  at,
}: {
  tag: string;
  text: string;
  at: string;
}) {
  return (
    <div className="flex items-center gap-[10px] border-b border-line-hair-3 px-[14px] py-[8px]">
      <LogTag value={tag} />
      <span className="min-w-0 flex-1 truncate font-mono text-sm text-secondary">
        {text}
      </span>
      <span className="shrink-0 font-mono text-2xs-plus text-faint">{at}</span>
    </div>
  );
}

// ── Toggle ──────────────────────────────────────────────────────────────────

export interface ToggleProps {
  checked: boolean;
  label: string;
  disabled?: boolean;
  /** Why it cannot be driven — surfaced as the control's title. */
  disabledReason?: string;
  onChange?: (next: boolean) => void;
}

export function Toggle({
  checked,
  label,
  disabled = false,
  disabledReason,
  onChange,
}: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      title={disabled ? disabledReason : undefined}
      onClick={() => onChange?.(!checked)}
      className={cn(
        "flex h-[19px] w-[34px] shrink-0 items-center rounded-pill border px-[2px]",
        "transition-[background-color,border-color] duration-[120ms]",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
        checked ? "border-green bg-green" : "border-line-strong bg-muted",
        disabled ? "cursor-not-allowed opacity-55" : "cursor-pointer",
        checked ? "justify-end" : "justify-start",
      )}
    >
      <span
        aria-hidden
        className="block h-[13px] w-[13px] rounded-full bg-raised"
      />
    </button>
  );
}

/** Loading / error / empty copy inside a `ListCard`, on the row grid. */
export function RowMessage({ children }: { children: React.ReactNode }) {
  return (
    <p className="m-0 px-[16px] py-[14px] text-md text-muted-fg">{children}</p>
  );
}

/**
 * The three states every server-backed list shares. Kept here so no section
 * invents its own wording — and so a failed request is always *said*, never
 * shown as an empty list.
 */
export function ListState({
  pending,
  error,
  empty,
  emptyCopy,
  children,
}: {
  pending: boolean;
  error: Error | null;
  empty: boolean;
  emptyCopy: string;
  children: React.ReactNode;
}) {
  if (pending) return <RowMessage>Loading…</RowMessage>;
  if (error !== null)
    return <RowMessage>Could not load — {error.message}</RowMessage>;
  if (empty) return <RowMessage>{emptyCopy}</RowMessage>;
  return <>{children}</>;
}
