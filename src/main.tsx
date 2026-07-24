import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { AppContextMenu } from "./components/AppContextMenu";
import { QuickWindow } from "./components/QuickWindow";
import { ConfirmDialogProvider } from "./components/ConfirmDialog";
import "./index.css";
import { applyColorScheme, applyFontScale, getCachedColorScheme, getCachedFontScale } from "./lib/uiPreferences";

applyColorScheme(getCachedColorScheme());
applyFontScale(getCachedFontScale());

const isQuickWindow = new URLSearchParams(window.location.search).get("window") === "quick";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ConfirmDialogProvider>
      {isQuickWindow ? <QuickWindow /> : <App />}
    </ConfirmDialogProvider>
    {isQuickWindow && <AppContextMenu />}
  </React.StrictMode>,
);
