import "@mantine/core/styles.css";
import "@mantine/charts/styles.css";
import "@mantine/notifications/styles.css";

import React from "react";
import ReactDOM from "react-dom/client";
import { MantineProvider, createTheme } from "@mantine/core";
import { Notifications } from "@mantine/notifications";

import { App } from "./App";

// LODForge's amber accent (the anvil icon), on Mantine's dark palette.
const theme = createTheme({
  primaryColor: "orange",
  defaultRadius: "md",
  fontFamily: "Segoe UI, system-ui, sans-serif",
  headings: { fontFamily: "Segoe UI, system-ui, sans-serif" },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <MantineProvider theme={theme} defaultColorScheme="dark">
      <Notifications position="bottom-right" />
      <App />
    </MantineProvider>
  </React.StrictMode>,
);
