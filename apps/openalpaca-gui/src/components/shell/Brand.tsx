/** `Brand` (DESIGN_SPEC §3.2) — the 22px mark and the wordmark. */

export function Brand() {
  return (
    <div className="mb-[22px] flex items-center gap-[9px] px-[6px]">
      <span
        aria-hidden
        className="flex h-[22px] w-[22px] items-center justify-center rounded-[5px] bg-ink"
      >
        <span className="block h-[7px] w-[7px] rounded-[1px] bg-canvas" />
      </span>
      <span className="text-lg font-semibold tracking-tight text-ink">
        OpenAlpaca
      </span>
    </div>
  );
}
