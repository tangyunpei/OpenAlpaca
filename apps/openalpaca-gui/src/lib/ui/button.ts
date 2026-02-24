import { cva, type VariantProps } from "class-variance-authority";

export const buttonVariants = cva(
  "inline-flex items-center justify-center whitespace-nowrap rounded-xl text-sm font-medium transition-all duration-250 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground border border-primary/50 hover:bg-accent-subtle hover:text-foreground hover:-translate-y-px hover:shadow-md",
        secondary:
          "bg-white/[0.04] text-muted-foreground border border-white/5 hover:bg-white/8 hover:text-foreground hover:border-white/10",
        ghost:
          "bg-transparent text-muted-foreground hover:bg-white/5 hover:text-foreground",
        destructive:
          "bg-danger/15 border border-danger/30 text-danger hover:bg-danger hover:text-danger-foreground hover:shadow-md",
        outline:
          "border border-border bg-transparent text-foreground hover:bg-white/5 hover:border-white/10",
        link: "text-accent underline-offset-4 hover:underline bg-transparent",
        accent:
          "bg-accent text-accent-foreground border border-accent/30 hover:brightness-110 hover:-translate-y-px hover:shadow-[0_4px_12px_hsl(40_85%_58%/0.2)]",
      },
      size: {
        sm: "h-8 px-3.5 text-xs",
        md: "h-9 px-5 py-2 text-sm",
        lg: "h-11 px-8 text-base",
        icon: "h-8 w-8 p-0",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "md",
    },
  },
);

export type ButtonVariants = VariantProps<typeof buttonVariants>;
