import type { Project } from "../api/types";

// Deterministic gradient from the project name so each project reads distinctly.
const GRADIENTS = [
  ["#4f8cff", "#22c3a6"],
  ["#a855f7", "#ec4899"],
  ["#f59e0b", "#ef4444"],
  ["#10b981", "#3b82f6"],
  ["#6366f1", "#06b6d4"],
  ["#f43f5e", "#f97316"],
];

function hash(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h;
}

export function ProjectAvatar({
  project,
  size = 40,
}: {
  project: { name: string; icon?: string | null };
  size?: number;
}) {
  const [c1, c2] = GRADIENTS[hash(project.name) % GRADIENTS.length];
  const initial = project.name.trim().charAt(0).toUpperCase() || "?";
  return (
    <div
      className="project-avatar"
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size / 3.4),
        background: `linear-gradient(135deg, ${c1}, ${c2})`,
        fontSize: Math.round(size * (project.icon ? 0.52 : 0.42)),
      }}
    >
      {project.icon ? project.icon : initial}
    </div>
  );
}

// Convenience for callers that only have loose data.
export type AvatarProject = Pick<Project, "name" | "icon">;
