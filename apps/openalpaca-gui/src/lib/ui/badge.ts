import { cva, type VariantProps } from "class-variance-authority";

export const badgeVariants = cva(
  "inline-flex items-center rounded-lg px-2 py-0.5 text-[0.65rem] font-bold uppercase border",
  {
    variants: {
      variant: {
        default: "bg-white/8 text-foreground border-white/5",
        muted: "bg-white/[0.03] text-muted-foreground border-white/[0.04]",
        success: "bg-success/12 text-success border-success/15",
        danger: "bg-danger/12 text-danger border-danger/15",
        warning: "bg-amber-500/12 text-amber-400 border-amber-400/15",
        accent: "bg-accent/10 text-accent border-accent/15",
        info: "bg-blue-400/12 text-blue-400 border-blue-400/15",
        purple: "bg-violet-500/12 text-violet-400 border-violet-400/15",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  },
);

export type BadgeVariants = VariantProps<typeof badgeVariants>;
