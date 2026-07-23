import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { AppContextMenu } from "./components/AppContextMenu";
import { QuickWindow } from "./components/QuickWindow";
import "./index.css";
import { applyColorScheme, getCachedColorScheme } from "./lib/uiPreferences";

applyColorScheme(getCachedColorScheme());

const isQuickWindow = new URLSearchParams(window.location.search).get("window") === "quick";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {isQuickWindow ? <QuickWindow /> : <App />}
    {isQuickWindow && <AppContextMenu />}
  </React.StrictMode>,
);
