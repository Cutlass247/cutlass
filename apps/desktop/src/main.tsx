import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { LicenseGate } from "./components/LicenseGate";
import "./styles.css";

// Outside the boundary, because a boundary only catches what its children
// throw while rendering: the gate is a child so its own crashes are caught,
// and anything the boundary itself needs is imported above, before render.
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <LicenseGate>
        <App />
      </LicenseGate>
    </ErrorBoundary>
  </React.StrictMode>
);
