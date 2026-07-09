import React from "react";
import ReactDOM from "react-dom/client";
import { AppProvider } from "./app/store";
import { ProjectsProvider } from "./app/projects";
import { ActivitiesProvider } from "./app/activities";
import { App } from "./app/App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <AppProvider>
      <ProjectsProvider>
        <ActivitiesProvider>
          <App />
        </ActivitiesProvider>
      </ProjectsProvider>
    </AppProvider>
  </React.StrictMode>
);
