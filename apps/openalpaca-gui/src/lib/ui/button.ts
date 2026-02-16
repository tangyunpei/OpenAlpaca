import { cva, type VariantProps } from "class-variance-authority";

export const buttonVariants = cva(
  "inline-flex items-center justify-center whitespace-nowrap rounded-lg text-sm font-medium transition-all duration-200 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground hover:bg-accent-subtle hover:text-foreground hover:-translate-y-px",
        secondary:
          "bg-white/5 text-muted-foreground hover:bg-white/10 hover:text-foreground",
        ghost:
          "bg-transparent text-muted-foreground hover:bg-white/5 hover:text-foreground",
        destructive:
          "bg-danger/20 border border-danger text-danger hover:bg-danger hover:text-danger-foreground",
        outline:
          "border border-border bg-transparent text-foreground hover:bg-white/5",
        link: "text-accent underline-offset-4 hover:underline bg-transparent",
        accent:
          "bg-accent text-accent-foreground hover:brightness-110 hover:-translate-y-px",
      },
      size: {
        sm: "h-8 px-3 text-xs",
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
