/**
 * The complete icon set (DESIGN_SPEC §6).
 *
 * Four inline SVGs — that is every icon in the design. No icon library is
 * installed on purpose: everything else the UI draws as an "icon" (`✓ ★ ☆ ‹ ›
 * ▴ ▾ ↗ ↵ ⏎ ⇧ ⌘ · — →`) is a text glyph rendered in IBM Plex and must stay
 * text so it inherits colour, size and font metrics.
 *
 * All four share `viewBox="0 0 24 24"`, `fill="none"`, `stroke="currentColor"`,
 * `stroke-width="1.8"` and are sized by the `size` prop: 15 in the nav rail,
 * 14 for the settings gear.
 */

export interface IconProps {
  /** Square px size. Nav rail uses 15; the settings gear uses 14. */
  size?: number;
  className?: string;
  /**
   * Icons here are always paired with a text label, so they are hidden from
   * assistive tech by default. Pass a `title` to expose one.
   */
  title?: string;
}

interface SvgProps extends IconProps {
  children: React.ReactNode;
}

function Svg({ size = 15, className, title, children }: SvgProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden={title === undefined}
      role={title === undefined ? undefined : "img"}
    >
      {title !== undefined && <title>{title}</title>}
      {children}
    </svg>
  );
}

/** Chat — speech bubble. */
export function ChatIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
    </Svg>
  );
}

/** Work — three stacked bars. */
export function WorkIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="3" y="4" width="18" height="4" rx="1" />
      <rect x="3" y="11" width="12" height="4" rx="1" />
      <rect x="3" y="18" width="15" height="3" rx="1" />
    </Svg>
  );
}

/** Library — folder. */
export function LibraryIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 4h5l2 3h9v13H4z" />
    </Svg>
  );
}

/** Settings — gear (circle + eight spokes). Drawn at 14 in the design. */
export function SettingsIcon({ size = 14, ...rest }: IconProps) {
  return (
    <Svg size={size} {...rest}>
      <circle cx="12" cy="12" r="3" />
      <path d="M12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M19 5l-2 2M7 17l-2 2" />
    </Svg>
  );
}
