/**
 * The toast slot (DESIGN_SPEC §3.34).
 *
 * All of the behaviour — one slot, 2600 ms, a new toast clearing the pending
 * timer — lives in `useUiStore.showToast`; this is only the binding between
 * that single value and the presentational `Toast`. Mount it once, as the last
 * child of the app frame, so its `absolute` positioning resolves against the
 * frame and its z-60 sits above the palette's z-50.
 */

import { Toast } from "@/components/ui";
import { useUiStore } from "@/stores/ui";

export function ToastHost() {
  const toast = useUiStore((s) => s.toast);
  if (toast === null) return null;
  return <Toast message={toast} />;
}
