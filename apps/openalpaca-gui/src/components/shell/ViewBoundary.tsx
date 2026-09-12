/**
 * Error boundary around the lazily-loaded views.
 *
 * Without it a failed chunk fetch (offline, stale build after an update) or a
 * render throw leaves the Suspense fallback on screen forever — an empty pane
 * with no explanation. This keeps the failure legible and recoverable.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  /** Remounts the boundary when the active view changes, clearing a stale error. */
  resetKey: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export class ViewBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidUpdate(prev: Props): void {
    if (prev.resetKey !== this.props.resetKey && this.state.error !== null) {
      this.setState({ error: null });
    }
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("View failed to render", error, info.componentStack);
  }

  override render(): ReactNode {
    const { error } = this.state;
    if (error === null) return this.props.children;

    return (
      <section className="flex min-w-0 flex-1 items-center justify-center bg-main px-8">
        <div className="max-w-[420px] text-center">
          <p className="text-sm-plus font-medium text-ink">
            This view failed to load
          </p>
          <p className="mt-2 font-mono text-xs text-muted-fg">
            {error.message}
          </p>
          <button
            type="button"
            onClick={() => this.setState({ error: null })}
            className="hover:bg-hover mt-4 rounded-xl border border-line bg-raised px-3 py-1.5 text-xs-plus text-ink transition-colors duration-[120ms]"
          >
            Try again
          </button>
        </div>
      </section>
    );
  }
}
