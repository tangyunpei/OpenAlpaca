<script lang="ts">
  import { cn } from "./cn";

  interface Props {
    checked?: boolean;
    onchange?: (e: Event) => void;
    disabled?: boolean;
    class?: string;
  }

  let { checked = false, onchange, disabled = false, class: className }: Props = $props();
</script>

<label class={cn("relative inline-flex h-6 w-11 shrink-0 cursor-pointer items-center", disabled && "opacity-50 cursor-not-allowed", className)}>
  <input
    type="checkbox"
    {checked}
    {onchange}
    {disabled}
    class="peer sr-only"
  />
  <span
    class="block h-6 w-11 rounded-full bg-primary transition-colors duration-300 peer-checked:bg-success"
  ></span>
  <span
    class="absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white transition-transform duration-300 peer-checked:translate-x-5"
  ></span>
</label>
