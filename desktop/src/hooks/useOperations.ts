import { useCallback, useEffect, useState } from "react";
import { api } from "../api/client";
import type { OperationRecord } from "../api/types";
import { useActivities } from "../app/activities";
import { useProjects } from "../app/projects";

export function useOperations(section: string) {
  const { tick } = useActivities();
  const { currentProject } = useProjects();
  const projectId = currentProject?.id ?? null;
  const [records, setRecords] = useState<OperationRecord[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    if (!projectId) {
      setRecords([]);
      setLoading(false);
      return;
    }
    try {
      const rows = await api.listOperations(projectId, section);
      setRecords(rows);
    } catch {
      setRecords([]);
    } finally {
      setLoading(false);
    }
  }, [projectId, section]);

  // Refetch on mount, on section/project change, and whenever a background
  // activity finishes (tick bumps).
  useEffect(() => {
    refresh();
  }, [refresh, tick]);

  return { records, loading, refresh, setRecords };
}
