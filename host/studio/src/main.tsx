import React from "react";
import ReactDOM from "react-dom/client";
import StudioRoot from "./StudioRoot";
import "./styles.css";

const rootElement = document.getElementById("root");
if (!rootElement) throw new Error("index.html has no #root element");

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <StudioRoot />
  </React.StrictMode>,
);
