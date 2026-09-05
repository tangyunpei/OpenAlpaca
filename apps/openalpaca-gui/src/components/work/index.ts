/**
 * The Work chunk (DESIGN_SPEC §3.18–§3.28).
 *
 * `WorkPane` is the cross-chunk seam the chat view imports; the artifact
 * renderers and the diff view are shared with the Library.
 */

export * from "./diff";
export * from "./DiffView";
export * from "./ParallelWork";
export * from "./preview";
export * from "./run-actions";
export * from "./run-events";
export * from "./run-model";
export * from "./RunActionBar";
export * from "./RunCard";
export * from "./useRunController";
export * from "./WorkListRow";
export * from "./WorkPane";
