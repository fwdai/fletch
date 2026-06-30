// Pane-level error boundary. A throw while rendering (e.g. a malformed agent
// transcript) would otherwise blank the whole window; this contains it to one
// pane and offers a reset, so the rest of the app keeps working. Keying the
// boundary (e.g. by agent id) makes switching agents reset it automatically.
import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./ui/Button";

interface Props {
  children: ReactNode;
  /** Human label for the failing region, e.g. "the workspace". */
  label?: string;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Surfaced to the console (and, in release, the injected Sentry browser SDK
    // captures it). React swallows the original throw once we render a fallback.
    console.error("UI error boundary caught an error:", error, info.componentStack);
  }

  reset = () => this.setState({ error: null });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="err-boundary" role="alert">
        <div className="err-boundary-box flex-center">
          <div className="err-boundary-title">
            Something went wrong{this.props.label ? ` in ${this.props.label}` : ""}.
          </div>
          <div className="err-boundary-msg">{error.message || String(error)}</div>
          <Button variant="outline" onClick={this.reset}>
            Try again
          </Button>
        </div>
      </div>
    );
  }
}
