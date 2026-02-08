import { cva, type VariantProps } from "class-variance-authority";

export const badgeVariants = cva(
  "inline-flex items-center rounded-full px-2 py-0.5 text-xs font-bold uppercase",
  {
    variants: {
      variant: {
        default: "bg-white/10 text-foreground",
        muted: "bg-white/5 text-muted-foreground",
        success: "bg-success/20 text-success",
        danger: "bg-danger/20 text-danger",
        warning: "bg-amber-500/20 text-amber-400",
        accent: "bg-accent/12 text-accent",
        info: "bg-blue-400/20 text-blue-400",
        purple: "bg-violet-500/20 text-violet-400",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  },
);

export type BadgeVariants = VariantProps<typeof badgeVariants>;
