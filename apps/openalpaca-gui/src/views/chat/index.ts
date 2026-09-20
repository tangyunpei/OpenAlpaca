/**
 * The chat view (DESIGN_SPEC §2.2, §5.1).
 *
 * `ChatView` is the default export so it can be `lazy()`-loaded from the app
 * root; the named exports are the seam the Work chunk plugs into.
 */

export { default as ChatView } from "./ChatView";
export type { ChatViewProps } from "./ChatView";
export { renderDefaultWorkPane } from "./WorkPaneSlot";
export type { WorkPaneRenderer, WorkPaneSlotProps } from "./WorkPaneSlot";
export * from "./transcript-model";
export { useChatSession } from "./useChatSession";
export type { ChatSession } from "./useChatSession";
