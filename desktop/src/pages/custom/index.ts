import type { FC } from "react";
import { Dashboard } from "./Dashboard";
import { Activities } from "./Activities";
import { LocalServer } from "./LocalServer";
import { GodotRuntime } from "./GodotRuntime";
import { SettingsPage } from "./SettingsPage";
import {
  PluginHelper,
  SdkHelper,
  CliBuilder,
  Docs,
  Feedback,
  CliInfo,
} from "./content";
import { Reports, Recommendations, BuildHistory, Logs } from "./aggregators";
import type { CustomPageProps } from "./types";

export const CUSTOM_PAGES: Record<string, FC<CustomPageProps>> = {
  Dashboard,
  Activities,
  PluginHelper,
  LocalServer,
  GodotRuntime,
  SdkHelper,
  CliBuilder,
  Reports,
  Recommendations,
  BuildHistory,
  Docs,
  Logs,
  Feedback,
  Settings: SettingsPage,
  CliInfo,
};
