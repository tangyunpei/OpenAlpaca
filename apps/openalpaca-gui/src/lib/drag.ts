/**
 * Whether a drag is carrying **files** (U5).
 *
 * The distinction matters twice, and both times it is the difference between a
 * working app and a broken one:
 *
 *   * the composer accepts a dropped file, and must not accept dragged *text*
 *     — cancelling a text drag's default is how you break dropping a selection
 *     into the textarea;
 *   * `AppShell` swallows a file dropped anywhere else. `tauri.conf.json` sets
 *     `dragDropEnabled: false` so the webview sees drops itself (which is what
 *     makes the composer's drop possible at all), and a webview's own default
 *     for a dropped file is to *navigate to it* — replacing the running app.
 *
 * `types` is the one thing readable during `dragover`: `DataTransfer.files` is
 * empty until the drop, by design.
 */
export function dragCarriesFiles(
  transfer: Pick<DataTransfer, "types"> | null | undefined,
): boolean {
  if (transfer === null || transfer === undefined) return false;
  return Array.from(transfer.types ?? []).includes("Files");
}
