// Self-hosted fonts (latin subset, weights 400/500/600) — DESIGN_SPEC §1.1.
// The Tauri CSP declares no `font-src`, so a CDN link would be blocked; these
// resolve to bundled woff2 files served from the app origin.
import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-500.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource/ibm-plex-mono/latin-600.css";

import "./styles.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";

const container = document.getElementById("root");
if (!container) {
  throw new Error('Root element "#root" not found in index.html');
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
