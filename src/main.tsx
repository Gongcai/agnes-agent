import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { applyColorScheme, getCachedColorScheme } from "./lib/uiPreferences";

applyColorScheme(getCachedColorScheme());

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
